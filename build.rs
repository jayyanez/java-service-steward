// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Embeds a Windows VERSIONINFO resource into `wrapper.exe` so that Explorer,
//! SmartScreen, and code-signing services see the product name and version.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    embed_version_info();
}

#[cfg(windows)]
fn embed_version_info() {
    let version = env!("CARGO_PKG_VERSION");
    let mut resource = winresource::WindowsResource::new();
    resource
        .set("ProductName", "Java Service Steward")
        .set(
            "FileDescription",
            "Java Service Steward service host for Java applications",
        )
        .set("CompanyName", "Java Service Steward Authors")
        .set(
            "LegalCopyright",
            "Copyright 2026 Java Service Steward Authors. Licensed under Apache-2.0 OR MIT.",
        )
        .set("OriginalFilename", "wrapper.exe")
        .set("InternalName", "wrapper")
        .set("ProductVersion", version)
        .set("FileVersion", version);
    if let Err(error) = resource.compile() {
        println!("cargo:warning=could not embed the Windows version resource: {error}");
    }
}
