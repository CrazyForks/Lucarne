pub mod args;
pub mod autostart;
pub mod doctor;
pub mod paths;
pub mod process;

pub use args::{parse, usage, AutostartCommand, Command, ParseError};

pub fn run(command: Command) -> Result<(), String> {
    match command {
        Command::RunDaemon => Err("daemon command is handled by lucarned".to_string()),
        Command::Init => Err("init command is handled by lucarned".to_string()),
        Command::Help => {
            print!("{}", args::usage());
            Ok(())
        }
        Command::Version => {
            println!("lucarned {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Paths => {
            let info = paths::current_path_info(None)?;
            print!("{}", paths::format_path_info(&info));
            Ok(())
        }
        Command::Doctor => doctor::run_doctor(),
        Command::Autostart(command) => run_autostart(command),
    }
}

fn run_autostart(command: AutostartCommand) -> Result<(), String> {
    match command {
        AutostartCommand::Install { start, bin } => {
            let info = paths::current_path_info(bin)?;
            let lucarned = info
                .lucarned
                .ok_or_else(|| "lucarned binary not found; pass --bin PATH".to_string())?;
            autostart::install(
                &autostart::AutostartPaths {
                    lucarned,
                    config_dir: info.config_dir,
                    log_dir: info.log_dir,
                },
                start,
            )
        }
        AutostartCommand::Uninstall { stop } => autostart::uninstall(stop),
        AutostartCommand::Start => autostart::start_service(),
        AutostartCommand::Stop => autostart::stop_service(),
        AutostartCommand::Status => autostart::status(),
    }
}
