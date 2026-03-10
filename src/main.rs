#[cfg(not(feature = "desktop"))]
mod cli {
    use std::path::PathBuf;

    use opencrate_bms::bridge::traits::PointSource;
    use opencrate_bms::platform::init_platform;

    pub async fn run() {
        let scenario_path = std::env::args()
            .nth(1)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("scenarios/small-office.json"));

        let profiles_dir = PathBuf::from("profiles");

        let (platform, mut bridges) = init_platform(&scenario_path, &profiles_dir)
            .await
            .unwrap_or_else(|e| {
                eprintln!("Failed to initialize platform: {e}");
                std::process::exit(1);
            });

        let loaded = &platform.model.loaded;
        println!("Scenario: {}", loaded.config.scenario.name);
        println!(
            "  Devices configured: {}",
            loaded.config.devices.len()
        );
        println!("  Devices loaded: {}", loaded.devices.len());

        if !loaded.warnings.is_empty() {
            println!("\nWarnings:");
            for w in &loaded.warnings {
                println!("  ⚠ {w}");
            }
        }

        println!("\nHistory collector started (data/history.db)");
        println!("Alarm engine started (data/alarms.db)");
        println!(
            "\nPoint store: {} points",
            platform.model.point_store.point_count()
        );
        println!("Press Ctrl+C to stop.");
        tokio::signal::ctrl_c().await.ok();

        println!("\nShutting down...");
        if let Some(ref mut b) = bridges.bacnet {
            b.stop().await.ok();
        }
        if let Some(ref mut m) = bridges.modbus {
            m.stop().await.ok();
        }
    }
}

#[cfg(not(feature = "desktop"))]
#[tokio::main]
async fn main() {
    cli::run().await;
}

#[cfg(feature = "desktop")]
fn main() {
    use opencrate_bms::gui::app::App;

    dioxus::launch(App);
}
