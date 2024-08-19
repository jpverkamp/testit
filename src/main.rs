use std::{collections::HashMap, io::Read, path, process::Command, time::Duration};

use clap::Parser;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use serde::{Deserialize, Serialize};
use wait_timeout::ChildExt;

/// Test a series of input files to check that output hasn't changed
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The command to run
    #[arg(short, long)]
    command: String,

    /// The directory to run the command in and store the output file in
    /// Defaults to the current directory
    #[arg(short, long)]
    directory: Option<String>,

    /// A glob style pattern defining the files to test
    #[arg(short, long)]
    files: String,

    /// Specify environment variables as key=value pairs; multiple can be specified
    #[arg(short, long)]
    env: Vec<String>,

    /// The database file that will store successful results (as a JSON file)
    #[arg(short = 'o', long)]
    db: Option<String>,

    /// If this flag is set, save new successes to the output file
    /// Defaults to false
    #[arg(short, long, action)]
    save: bool,

    /// The time to allow for each test
    /// Defaults to 1 second
    #[arg(short, long)]
    timeout: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
enum TestResult {
    Success(String),
    Failure(String),
    Timeout,
}

fn main() {
    let args = Args::parse();

    // Build the absolute glob pattern
    // This is based on the working directory (or cwd) from the args + the files pattern
    let pattern = format!(
        "{}/{}",
        args.directory.clone().unwrap_or(".".to_string()),
        args.files
    );
    
    // Glob the list of all files that we want to test
    let files = glob::glob(&pattern)
        .unwrap()
        .map(|x| x.unwrap())
        .collect::<Vec<path::PathBuf>>();

    // Calculate the output file path (if specified; apply directory if specified)
    let db_path = if let Some(output) = args.db.clone() {
        if let Some(prefix) = args.directory.clone() {
            Some(format!("{}/{}", prefix, output))
        } else {
            Some(output)
        }
    } else {
        None
    };

    // --save doesn't make sense without --output
    if args.save && db_path.is_none() {
        panic!("--save requires --output to be set");
    }

    // Parse environment variables
    // There should be exactly one =
    let env: HashMap<String, String> = args.env.iter().map(|x| {
        assert!(x.matches('=').count() == 1);
        let mut split = x.split("=");
        (split.next().unwrap().to_string(), split.next().unwrap().to_string())
    }).collect();

    // For each file, run the command and compare the output
    let results = files.par_iter().map(|file| {
        println!("Testing {}", file.display());

        let command = args.command.clone();
        let cwd = args.directory.clone().unwrap_or(".".to_string());
        let stdin = std::fs::File::open(&file).unwrap();
        let timeout = Duration::from_secs(args.timeout.unwrap_or(10));

        // Create the child process
        let mut command_builder = Command::new("bash");
        command_builder
            .arg("-c")
            .arg(command)
            .current_dir(&cwd)
            .stdin(stdin)
            .stderr(std::process::Stdio::piped())  // TODO: Do we want to capture this?
            .stdout(std::process::Stdio::piped());

        // Add environment variables
        for (key, value) in env.iter() {
            command_builder.env(key, value);
        }

        // Start the child
        let mut child = command_builder
            .spawn()
            .expect("Failed to execute command");

        // Wait for the child to finish up to timeout
        // If timeout is reached, kill the thread (or it may outlast us...)
        match child.wait_timeout(timeout) {
            Ok(Some(status)) => {
                let mut output = String::new();
                child.stdout.as_mut().unwrap().read_to_string(&mut output).unwrap();

                if status.success() {
                    TestResult::Success(output)
                } else {
                    TestResult::Failure(output)
                }
            }
            Ok(None) => {
                // Timeout passed without exit
                child.kill().unwrap();
                TestResult::Timeout
            }
            Err(_) => {
                // Process errored out
                child.kill().unwrap();
                unimplemented!("Process errored out")
            },
        }
    }).collect::<Vec<_>>();

    // If we have a previous output file, compare results
    let mut previous_results: HashMap<String, Vec<String>> = if let Some(output_file_path) = db_path.clone() {        
        if let Ok(f) = std::fs::read_to_string(output_file_path) {
            serde_json::from_str(&f).unwrap()
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };

    // Write results
    // This will only print failures, timeouts, and new successes
    // If the output file is set and we see the same success again, it will be ignored
    for (file, result) in files.iter().zip(results.iter()) {
        // Remove the directory prefix if it exists
        // This will apply to the printed output + the output file
        let file = if let Some(prefix) = args.directory.clone() {
            file.strip_prefix(prefix).unwrap()
        } else {
            file
        };

        match result {
            TestResult::Success(output) => {
                if let Some(previous) = previous_results.get(file.to_str().unwrap()) {
                    if previous.contains(output) {
                        // We have a previously logged success, do nothing
                        continue;
                    }
                }

                // We have successful output we haven't seen before, log it and potentially save it
                println!("{}: New success:\n{}\n===\n", file.display(), output);
                if args.save {
                    previous_results
                        .entry(file.to_str().unwrap().to_string())
                        .or_insert(Vec::new())
                        .push(output.clone());
                }
            }
            TestResult::Failure(output) => {
                println!("{}: Failure\n{}\n===\n", file.display(), output);
            }
            TestResult::Timeout => {
                println!("{}: Timeout", file.display());
            }
        }
    }

    // Save the new results (if requested)
    if args.save {
        let f = std::fs::File::create(db_path.expect("Tried to save with no output file")).unwrap();
        serde_json::to_writer_pretty(f, &previous_results).unwrap();
    }
}
