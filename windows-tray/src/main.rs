#![cfg_attr(windows, windows_subsystem = "windows")]
mod backend;
mod control;
mod credentials;
#[cfg(windows)]
mod panel;
#[cfg(windows)]
mod setup;
#[cfg(windows)]
mod tray;
fn main() {
    #[cfg(windows)]
    if std::env::args().nth(1).as_deref() == Some("--preview") {
        if let Some(path) = std::env::args_os().nth(2) {
            if panel::preview(std::path::Path::new(&path)).is_err() {
                std::process::exit(1);
            }
        }
        return;
    }

    if std::env::args().nth(1).as_deref() == Some("--check") {
        let result = (|| -> Result<String, String> {
            let path = std::env::args_os()
                .nth(2)
                .ok_or("Usage: bulb-tray --check client.json")?;
            let config = serde_json::from_slice(
                &std::fs::read(path).map_err(|_| "Cannot read client configuration")?,
            )
            .map_err(|_| "Invalid client configuration")?;
            backend::Controller::new(config)?.status()
        })();
        match result {
            Ok(status) => println!("{status}"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    #[cfg(windows)]
    if let Err(message) = tray::run() {
        tray::show_error(&message);
    }
    #[cfg(not(windows))]
    eprintln!("bulb-tray is a Windows application. Build bulb-server for Linux.");
}
