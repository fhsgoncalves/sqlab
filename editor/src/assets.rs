use gpui::{AssetSource, Result, SharedString};
use gpui_component_assets::Assets as GpuiAssets;
use std::borrow::Cow;

#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
struct LocalAssets;

#[derive(rust_embed::RustEmbed)]
#[folder = "../assets"]
#[include = "icons/**/*.svg"]
struct WorkspaceAssets;

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = LocalAssets::get(path) {
            return Ok(Some(file.data));
        }
        if let Some(file) = WorkspaceAssets::get(path) {
            return Ok(Some(file.data));
        }
        GpuiAssets::new("").load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut results: Vec<SharedString> = LocalAssets::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect();
        results
            .extend(WorkspaceAssets::iter().filter_map(|p| p.starts_with(path).then(|| p.into())));
        results.extend(GpuiAssets::new("").list(path)?);
        Ok(results)
    }
}
