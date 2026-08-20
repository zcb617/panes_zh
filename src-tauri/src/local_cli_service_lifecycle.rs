use crate::{
    commands::harness::{detect_harness, detect_via_login_shell, HARNESSES},
    models::HarnessReport,
    runtime_env,
};

pub(crate) struct LocalCliServiceLifecycle;

impl LocalCliServiceLifecycle {
    pub(crate) async fn list_ready() -> Result<HarnessReport, String> {
        let mut harnesses = Vec::new();

        for def in HARNESSES {
            let status = detect_harness(def).await;
            harnesses.push(status);
        }

        let package_manager_available = runtime_env::resolve_executable("npm").is_some()
            || detect_via_login_shell("npm", "--version").await.is_some();

        let mise_preferred =
            runtime_env::is_flatpak() && runtime_env::resolve_executable("mise").is_some();
        let preferred_install_method = if mise_preferred {
            Some("mise".to_string())
        } else if package_manager_available {
            Some("npm".to_string())
        } else {
            None
        };

        Ok(HarnessReport {
            harnesses,
            npm_available: package_manager_available,
            preferred_install_method,
        })
    }
}
