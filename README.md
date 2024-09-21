A (hopefully) simple tool to test many input files against a program that runs stdin -> stdout. 

There are currently three modes:

* `testit run [options] <command> <files>` - Run a command against a series of files (as a glob pattern)
* `testit record [options] <command> <files> <db>` - The same as above, but save the output and options used to `<db>` for later use. 
* `testit update [options] <db>` - Load a previously saved DB and re-run the `command` and `files` used in that. Any options specified here will be used instead and saved for later. 

# Options

Here are the options used for all modes:

```
-d, --directory <DIRECTORY>
    The working directory to run the command from (default: cwd)

--stdout-mode <STDOUT_MODE>
    How to direct stdout (default: both)

    Possible values:
    - none:  Don't save or print
    - save:  Save to database, don't print
    - print: Print as normal, but don't save
    - both:  Save to database and print

--stderr-mode <STDERR_MODE>
    How to direct stderr (default: print); same values as stdout-mode

-e, --env <ENV>
    Specify environment variables as key=value pairs; multiple can be specified (default: [])

-E, --preserve-env <PRESERVE_ENV>
    Preserve the environment of the parent process (default: false)

-t, --timeout <TIMEOUT>
    The time to allow for each test in seconds (default: 10)

-v, --verbose...
        Increase logging verbosity

-q, --quiet...
        Decrease logging verbosity
```

# Global options

Here are options that control the running of the entire program:

```
-v, --verbose...
      Increase logging verbosity. `-vv` will print progress as tasks run; `-vvv` will print the options used. 

-q, --quiet...
      Decrease logging verbosity. `-q` will not print anything; although the status code might still be useful. 

-n, --dry-run
      If this flag is set, don't automatically save to the database (if set); does nothing in `run` mode

-h, --help
      Print help (see a summary with '-h')
```

# Verbosity

* `-v` doesn't currently print anything (we have no warnings)
* `-vv` prints each task as it starts and finishes
* `-vvv` also prints a periodic progress notification (with exponential decay up to 30s)