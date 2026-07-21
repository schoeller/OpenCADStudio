// build.rs — Windows executable icon embedding.
//
// The core ribbon module registry used to be generated here; it is now a
// hand-written static list in src/modules/registry.rs.

#[cfg(windows)]
use std::path::Path;

fn main() {
    // The Patreon token is baked in at compile time via `option_env!` in
    // src/patreon.rs. `option_env!` is not tracked by Cargo, so without this a
    // token change wouldn't trigger a rebuild — declare the dependency so an
    // updated OCS_PATREON_TOKEN re-bakes the binary. (#229-adjacent)
    println!("cargo:rerun-if-env-changed=OCS_PATREON_TOKEN");

    // Windows: embed AppIcon.ico into the .exe so the executable carries its
    // own icon (Explorer, taskbar, Start-menu tile, file associations). The
    // .ico is produced from assets/logo.svg by the release workflow before the
    // build; when it is absent (local/dev builds) this is skipped. See #107.
    //
    // The Start page (`src/app/view/mod.rs`) builds a very large widget tree in
    // one function; in unoptimized debug builds the compiler keeps many large
    // `Element` values on the stack and the default 1 MiB Windows stack is not
    // enough. Bump the executable stack reservation to 8 MiB on Windows so local
    // debug runs start reliably. (#312)
    #[cfg(windows)]
    {
        println!("cargo:rustc-link-arg=/STACK:8388608");
    }
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=packaging/windows/AppIcon.ico");
        if Path::new("packaging/windows/AppIcon.ico").exists() {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("packaging/windows/AppIcon.ico");
            if let Err(e) = res.compile() {
                println!("cargo:warning=failed to embed Windows icon: {e}");
            }
        }
    }
}
