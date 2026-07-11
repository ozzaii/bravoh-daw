//! bravoh-daw CLI: parse DAW project files and print the result as JSON.

use std::process::ExitCode;

const USAGE: &str = "\
bravoh-daw — parse DAW project files into unified JSON

USAGE:
    bravoh-daw <PROJECT_FILE>...

Supported formats: .als (Ableton Live), .flp (FL Studio),
                   .logicx (Logic Pro), .rpp (REAPER)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!("{USAGE}");
        return if args.is_empty() {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        };
    }

    let mut failed = false;
    for arg in &args {
        match bravoh_daw::parse(arg) {
            Ok(intel) => match serde_json::to_string_pretty(&intel) {
                Ok(json) => println!("{json}"),
                Err(e) => {
                    eprintln!("error: failed to serialize {arg}: {e}");
                    failed = true;
                }
            },
            Err(e) => {
                eprintln!("error: {e}");
                failed = true;
            }
        }
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
