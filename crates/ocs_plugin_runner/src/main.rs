fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: ocs_plugin_runner <sync-socket> <async-socket> <cdylib>");
        std::process::exit(1);
    }
    if let Err(e) = ocs_plugin_api::runner::run(&args[1], &args[2], std::path::Path::new(&args[3]))
    {
        eprintln!("[runner] fatal: {e}");
        std::process::exit(1);
    }
}
