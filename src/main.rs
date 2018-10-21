extern crate playloop;

use std::env;
use std::process;

fn main() {
	let args: Vec<String> = env::args().collect();
	if args.len() < 2 {
		println!("usage: playloop [file]");
		process::exit(1);
	}

	if let Err(e) = playloop::run(&args[1]) {
		println!("error: {}", e);
		process::exit(1);
	}
}
