// SPDX-License-Identifier: Apache-2.0 OR MIT
pub mod cli;
pub mod config;
pub mod diagnostics;
pub mod error;
pub mod heap_dump;
pub mod jvm;
pub mod logging;
pub mod protocol;
pub mod service;
pub mod supervisor;
pub mod telemetry;
pub mod thread_dump;
pub mod windows_process;

pub const PRODUCT_NAME: &str = "Java Service Steward";
pub const PRODUCT_DESCRIPTION: &str =
    "Runs, controls, and monitors Java applications as Windows services.";
pub const CONFIG_FORMAT_NOTE: &str = "Reads wrapper.conf-style configuration files.";
pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Terms that must never appear in the product's own output (banners, help
/// text, log records): the name of the company behind the original Java
/// Service Wrapper, its documentation host, and its version series. Tests use
/// this list as a guard; it is not part of the supported API.
#[doc(hidden)]
pub const FOREIGN_NAME_MARKERS: &[&str] = &["Tanuki", "tanukisoftware.com", "3.5."];

#[cfg(target_pointer_width = "64")]
pub const PRODUCT_ARCHITECTURE: &str = "64-bit";

#[cfg(target_pointer_width = "32")]
pub const PRODUCT_ARCHITECTURE: &str = "32-bit";

/// First line of `--version`, `--help` and every log file.
#[must_use]
pub fn product_banner() -> String {
    format!("{PRODUCT_NAME} {PRODUCT_ARCHITECTURE} {PRODUCT_VERSION}")
}

/// Complete `--version` output: banner, description and configuration note.
#[must_use]
pub fn version_text() -> String {
    format!(
        "{}\n{PRODUCT_DESCRIPTION}\n{CONFIG_FORMAT_NOTE}\n",
        product_banner()
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn product_identity_is_stable() {
        assert_eq!(crate::PRODUCT_NAME, "Java Service Steward");
        assert_eq!(
            crate::PRODUCT_DESCRIPTION,
            "Runs, controls, and monitors Java applications as Windows services."
        );
    }

    #[test]
    fn version_text_names_no_third_party_product_or_version() {
        let text = crate::version_text();
        assert_eq!(text.lines().count(), 3);
        assert!(text.starts_with(&crate::product_banner()));
        for forbidden in crate::FOREIGN_NAME_MARKERS.iter().chain(&["Copyright"]) {
            assert!(
                !text.contains(forbidden),
                "version text must not contain {forbidden}"
            );
        }
    }
}
