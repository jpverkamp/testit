```
Test a series of input files to check that output hasn't changed

Usage: testit [OPTIONS] --command <COMMAND> --files <FILES>

Options:
  -c, --command <COMMAND>      The command to run
  -d, --directory <DIRECTORY>  The directory to run the command in and store the output file in Defaults to the current directory
  -f, --files <FILES>          A glob style pattern defining the files to test
  -e, --env <ENV>              Specify environment variables as key=value pairs; multiple can be specified
  -o, --db <OUTPUT>            The database file that will store successful results (json)
  -s, --save                   If this flag is set, save new successes to the output file Defaults to false
  -t, --timeout <TIMEOUT>      The time to allow for each test Defaults to 1 second
  -h, --help                   Print help
  -V, --version                Print version
```

Example:

```
cargo run -- \
  --command "cargo run --release --bin cosmic-express" \
  --env COSMIC_EXPRESS_FLOODFILL_VALIDATOR=true \
  --env COSMIC_EXPRESS_HEURISTIC_NEAREST_HOUSE=true \
  --directory ../rust-solvers/ \
  --files "data/cosmic-express/**/*.json" \
  --timeout 60 \
  --db "results.json" \
  --save
```

Running the test cases from my [Rust Solvers](https://github.com/jpverkamp/rust-solvers) repo. 

General usage will be:

* Initial run:
  * Run without `--save` to see if the results are reasonable
  * Re-run with both of those flags

* After changes:
  * Run without `--save`
  * If all tests still pass: awesome!
  * If any tests pass with new output, verify it and `--save`