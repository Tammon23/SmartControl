use bulb_server::{random_key, router, App, Config};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let argument = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.json".into());
    if argument == "keygen" {
        println!("{}", random_key());
        return Ok(());
    }
    if argument == "init" {
        use std::io::Write;
        let path = std::env::args()
            .nth(2)
            .unwrap_or_else(|| "config.json".into());
        let value = serde_json::json!({"bind":"127.0.0.1:8787","demo":true,"clients":[{"name":"windows","token":random_key()},{"name":"kindle","token":random_key()}],"admin_token":random_key(),"device_id":"00000000-0000-0000-0000-000000000000","component":"main","tokens_file":"data/tokens.json","oauth":null});
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&path)?
            .write_all(&serde_json::to_vec_pretty(&value)?)?;
        println!("Created {path} in demo mode. Existing files are never overwritten.");
        return Ok(());
    }
    let mut config: Config = serde_json::from_slice(&tokio::fs::read(argument).await?)?;
    if let Ok(bind) = std::env::var("BULB_BIND") {
        config.bind = bind;
    }
    let bind = config.bind.clone();
    let demo = config.demo;
    let app = App::new(config).await.map_err(std::io::Error::other)?;
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    println!(
        "Bulb Bridge listening on {} ({})",
        listener.local_addr()?,
        if demo {
            "DEMO: no bulb commands"
        } else {
            "SmartThings cloud"
        }
    );
    axum::serve(listener, router(app))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
