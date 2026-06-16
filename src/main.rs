use clap::Parser;

#[derive(Parser)]
struct Args {
    /// Directory to watch
    #[arg(short, long, default_value_t = String::from("."))]
    dir: String,

    /// Glob patterns to ignore
    #[arg(short, long)]
    ignore: Vec<String>,

    /// Include hidden
    #[arg(long, default_value_t = false)]
    hidden: bool,

    /// Command to run after `--`
    command: Vec<String>
}

fn main() {
    let args = Args::parse();
    whatchanged::run(args.dir, args.ignore, args.command, args.hidden).expect("Error")
}
