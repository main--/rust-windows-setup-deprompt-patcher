extern crate windows_setup_deprompt_patcher;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <path to windows iso>", args[0]);
        return;
    }

    match windows_setup_deprompt_patcher::patch(&args[1], false).unwrap() {
        true => println!("Patched!"),
        false => println!("Nothing to do."),
    }
}
