use std::collections::BTreeMap;
use std::io::Read;
use std::path;
use std::process::Command;
use std::time::Duration;

use clap::Parser;
use clap_verbosity_flag::Verbosity;
use env_logger;
use log;
use rayon::iter::ParallelIterator;
use rayon_progress::ProgressAdaptor;
use serde::{Deserialize, Serialize};
use wait_timeout::ChildExt;

/// Test a series of input files to check that output hasn't changed
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The mode to run in
    #[clap(subcommand)]
    mode: Mode,

    /// Varying levels of verbosity from -q (quiet) to -vvvv (warning, info, debug, trace)
    #[command(flatten)]
    verbose: Verbosity,

    /// If this flag is set, don't automatically save to the database (if set)
    #[arg(short = 'n', long, action, global = true)]
    dry_run: bool,
}

// Options that are saved with record and cannot be overridden
#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
struct Metadata {
    /// The command to run; should read from stdin and write to stdout and/or stderr
    command: String,

    /// The working directory to run the command from (default: cwd)
    #[arg(short, long)]
    directory: Option<String>,

    /// A glob style pattern defining the files to test
    files: String,
}

// Global options
#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
struct Options {
    /// How to direct stdout (default: both)
    #[arg(long)]
    stdout_mode: Option<StreamMode>,

    /// How to direct stderr (default: print)
    #[arg(long)]
    stderr_mode: Option<StreamMode>,

    /// Specify environment variables as key=value pairs; multiple can be specified (default: [])
    #[arg(short, long)]
    env: Vec<String>,

    /// Preserve the environment of the parent process (default: false)
    #[arg(short = 'E', long)]
    preserve_env: Option<bool>,

    /// The time to allow for each test in seconds (default: 10)
    #[arg(short, long)]
    timeout: Option<u64>,
}

// Subcommands
#[derive(Parser, Debug, Clone)]
enum Mode {
    /// Run without a database
    Run {
        #[clap(flatten)]
        metadata: Metadata,

        #[clap(flatten)]
        options: Options,
    },

    /// Record new input with the given options.
    Record {
        #[clap(flatten)]
        metadata: Metadata,

        /// The database file to save to
        db: String,

        #[clap(flatten)]
        options: Options,
    },

    /// Reun/update the given db file; new options will also be saved.
    Update {
        /// The database file to run
        db: String,

        #[clap(flatten)]
        options: Options,
    },
}

#[derive(Debug, Clone, clap::ValueEnum, Serialize, Deserialize)]
enum StreamMode {
    /// Don't save or print
    None,

    /// Save to database, don't print
    Save,

    /// Print as normal, but don't save
    Print,

    /// Save to database and print
    Both,
}

impl std::fmt::Display for StreamMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamMode::None => write!(f, "none"),
            StreamMode::Save => write!(f, "save"),
            StreamMode::Print => write!(f, "print"),
            StreamMode::Both => write!(f, "both"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum TestResult {
    Success(String, String, u128),
    Failure(String, String),
    Timeout,
}

#[derive(Debug, Serialize, Deserialize)]
struct TimingData {
    fastest: u128,
    most_recent: u128,
}

#[derive(Debug, Serialize, Deserialize)]
struct Db {
    results: BTreeMap<String, Vec<String>>,

    #[serde(alias = "%metadata%")]
    metadata: Metadata,

    #[serde(alias = "%options%")]
    options: Options,

    #[serde(alias = "%timing%", default)]
    timing: BTreeMap<String, TimingData>,
}

fn main() {
    let args = Args::parse();
    env_logger::Builder::new()
        .filter_level(args.verbose.log_level_filter())
        .init();

    log::warn!("Logs are only available at -v and -vv");

    // Load options
    macro_rules! override_option {
        ($db:expr, $args:expr, $field:ident) => {
            if let Some(value) = &$args.$field {
                $db.options.$field = Some(value.clone());
            }
        };
    }

    // 1) Set values from the mode + defaults
    let mut db = match &args.mode {
        Mode::Run { metadata, options }
        | Mode::Record {
            metadata, options, ..
        } => Db {
            results: BTreeMap::new(),
            metadata: metadata.clone(),
            options: options.clone(),
            timing: BTreeMap::new(),
        },
        Mode::Update { db, options } => {
            // File doesn't exist
            if !std::path::Path::new(db).exists() {
                eprintln!("Database file does not exist: {}", db);
                std::process::exit(1);
            }

            let f = std::fs::File::open(db).unwrap();
            let mut db: Db = serde_json::from_reader(f).unwrap();

            // 2) Override db values with values from the command line
            override_option!(db, options, stdout_mode);
            override_option!(db, options, stderr_mode);
            override_option!(db, options, preserve_env);
            override_option!(db, options, timeout);

            // Env is a vec, so set it only if it's not empty
            if !options.env.is_empty() {
                db.options.env = options.env.clone();
            }

            db
        }
    };

    // 3) Replace any unset values with their defaults
    if db.options.stdout_mode.is_none() {
        db.options.stdout_mode = Some(StreamMode::Both);
    }
    if db.options.stderr_mode.is_none() {
        db.options.stderr_mode = Some(StreamMode::Print);
    }
    if db.options.preserve_env.is_none() {
        db.options.preserve_env = Some(false);
    }
    if db.options.timeout.is_none() {
        db.options.timeout = Some(10);
    }

    // Debug print options
    log::debug!("Options:\n{:#?}\n{:#?}", db.metadata, db.options);

    // Build the absolute glob pattern
    // This is based on the working directory (or cwd) from the args + the files pattern
    let pattern = format!(
        "{}/{}",
        db.metadata
            .directory
            .clone()
            .unwrap_or_else(|| ".".to_string()),
        db.metadata.files
    );

    // Glob the list of all files that we want to test
    let files = glob::glob(&pattern)
        .unwrap()
        .map(|x| x.unwrap())
        .collect::<Vec<path::PathBuf>>();

    // Parse environment variables
    // There should be exactly one =
    let env: BTreeMap<String, String> = db
        .options
        .env
        .iter()
        .map(|x| {
            assert!(x.matches('=').count() == 1);
            let mut split = x.split("=");
            (
                split.next().unwrap().to_string(),
                split.next().unwrap().to_string(),
            )
        })
        .collect();

    // Progress adaptor
    let it = ProgressAdaptor::new(&files);
    let progress = it.items_processed();
    let total = files.len();
    let start = std::time::Instant::now();

    // Additional thread that displays progress over time
    std::thread::spawn(move || {
        let mut last_progress = 0;
        let mut last_print = std::time::Instant::now();
        let mut delay = 1000;

        loop {
            std::thread::sleep(Duration::from_millis(1000));

            let new_progress = progress.get();
            let time_spent = start.elapsed().as_secs();

            if new_progress != last_progress {
                
                // Made progress, reset delay
                log::debug!("Progress: {}/{} files, {}/{} sec (max)", new_progress, total, time_spent, db.options.timeout.unwrap());
                last_print = std::time::Instant::now();
                delay = 1000;
                last_progress = new_progress;
                
            } else if last_print.elapsed().as_millis() > delay {
                
                // Met delay, print and increment delay
                log::debug!("Progress: {}/{} files, {}/{} sec (max)", new_progress, total, time_spent, db.options.timeout.unwrap());
                last_print = std::time::Instant::now();
                delay = 30000.min(delay * 2);

            }
        }
    });

    // For each file, run the command and compare the output
    let results = it
        .map(|file| {
            log::info!("Testing {}", file.display());
            let start = std::time::Instant::now();

            let command = db.metadata.command.clone();
            let cwd = db.metadata.directory.clone();
            let stdin = std::fs::File::open(&file).unwrap();
            let timeout = Duration::from_secs(db.options.timeout.unwrap());

            // Create the child process
            let mut command_builder = Command::new("bash");
            command_builder
                .arg("-c")
                .arg(command)
                .current_dir(&cwd.unwrap_or_else(|| ".".to_string()))
                .stdin(stdin)
                .stderr(std::process::Stdio::piped()) // TODO: Do we want to capture this?
                .stdout(std::process::Stdio::piped());

            // Add environment variables
            if !db.options.preserve_env.unwrap() {
                command_builder.env_clear();
            }
            for (key, value) in env.iter() {
                command_builder.env(key, value);
            }

            // Start the child
            let mut child = command_builder.spawn().expect("Failed to execute command");

            // Wait for the child to finish up to timeout
            // If timeout is reached, kill the thread (or it may outlast us...)
            match child.wait_timeout(timeout) {
                Ok(Some(status)) => {
                    let mut output = String::new();
                    child
                        .stdout
                        .as_mut()
                        .unwrap()
                        .read_to_string(&mut output)
                        .unwrap();

                    let mut error = String::new();
                    child
                        .stderr
                        .as_mut()
                        .unwrap()
                        .read_to_string(&mut error)
                        .unwrap();

                    if status.success() {
                        let elapsed = start.elapsed().as_millis();
                        log::info!("Success after {}ms: {}", elapsed, file.display());
                        TestResult::Success(output, error, elapsed)
                    } else {
                        log::info!("Failure {}", file.display());
                        TestResult::Failure(output, error)
                    }
                }
                Ok(None) => {
                    // Timeout passed without exit
                    log::info!("Timeout {}", file.display());
                    child.kill().unwrap();
                    TestResult::Timeout
                }
                Err(_) => {
                    // Process errored out
                    child.kill().unwrap();
                    unimplemented!("Process errored out")
                }
            }
        })
        .collect::<Vec<_>>();

    let mut success_count = 0;
    let mut new_success_count = 0;
    let mut failure_count = 0;
    let mut timeout_count = 0;

    // Write results
    // This will only print failures, timeouts, and new successes
    // If the output file is set and we see the same success again, it will be ignored
    for (file, result) in files.iter().zip(results.iter()) {
        // Remove the directory prefix if it exists
        // This will apply to the printed output + the output file
        let file = if let Some(prefix) = db.metadata.directory.clone() {
            file.strip_prefix(prefix).unwrap()
        } else {
            file
        };

        match result {
            TestResult::Success(output, error, elapsed_ms) => {
                success_count += 1;

                // TODO: This is ugly, fix it with a function or something

                let mut to_print = String::new();
                match db.options.stdout_mode {
                    Some(StreamMode::Print) | Some(StreamMode::Both) => {
                        to_print.push_str(&output);
                    }
                    _ => {}
                }
                match db.options.stderr_mode {
                    Some(StreamMode::Print) | Some(StreamMode::Both) => {
                        to_print.push_str(&error);
                    }
                    _ => {}
                }

                let mut to_save = String::new();
                match db.options.stdout_mode {
                    Some(StreamMode::Save) | Some(StreamMode::Both) => {
                        to_save.push_str(&output);
                    }
                    _ => {}
                }
                match db.options.stderr_mode {
                    Some(StreamMode::Save) | Some(StreamMode::Both) => {
                        to_save.push_str(&error);
                    }
                    _ => {}
                }
            
                // Update timing data, even if we have a previous success
                let timing_data = db.timing
                    .entry(file.to_str().unwrap().to_string())
                    .or_insert(TimingData {
                        fastest: *elapsed_ms,
                        most_recent: *elapsed_ms,
                    });

                timing_data.most_recent = *elapsed_ms;
                timing_data.fastest = timing_data.fastest.min(*elapsed_ms);

                // Don't update results if we've already seen it
                if let Some(previous) = db.results.get(file.to_str().unwrap()) {
                    if previous.contains(&to_save) {
                        // We have a previously logged success, do nothing
                        continue;
                    }
                }
                new_success_count += 1;

                // We have successful output we haven't seen before, log it and potentially save it
                if !args.verbose.is_silent() {
                    println!("{}: New success:\n{}\n===\n", file.display(), to_print);
                }

                db.results
                    .entry(file.to_str().unwrap().to_string())
                    .or_insert(Vec::new())
                    .push(to_save.clone());
            }
            TestResult::Failure(output, error) => {
                // TODO: This is ugly, fix it with a function or something

                let mut to_print = String::new();
                match db.options.stdout_mode {
                    Some(StreamMode::Print) | Some(StreamMode::Both) => {
                        to_print.push_str(&output);
                    }
                    _ => {}
                }
                match db.options.stderr_mode {
                    Some(StreamMode::Print) | Some(StreamMode::Both) => {
                        to_print.push_str(&error);
                    }
                    _ => {}
                }

                failure_count += 1;

                if !args.verbose.is_silent() {
                    println!("{}: Failure\n{}\n===\n", file.display(), to_print);
                }
            }
            TestResult::Timeout => {
                timeout_count += 1;

                if !args.verbose.is_silent() {
                    println!("{}: Timeout", file.display());
                }
            }
        }
    }

    // Save the new results (if requested)
    if !args.dry_run {
        if let Some(db_path) = match args.mode {
            Mode::Record { db, .. } => Some(db),
            Mode::Update { db, .. } => Some(db),
            _ => None,
        } {
            let f = std::fs::File::create(db_path).expect("Unable to write to db file: {db_path}");
            serde_json::to_writer_pretty(f, &db).unwrap();
        }
    }

    // Output a summary
    if !args.verbose.is_silent() {
        println!(
            "\nSummary:\n\tSuccesses: {} ({} new)\n\tFailures: {}\n\tTimeouts: {}",
            success_count, new_success_count, failure_count, timeout_count
        );
    }

    // Exit a success if there were no failures or timeouts
    if failure_count == 0 && timeout_count == 0 {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}
