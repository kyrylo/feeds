use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

fn files(root: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for entry in fs::read_dir(root).unwrap_or_else(|e| panic!("{}: {e}", root.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.push(path);
        }
    }
    result.sort();
    result
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.parent().unwrap();
    let mut assets = Vec::new();
    let mut styles = Vec::new();
    let mut imports = BTreeMap::new();
    let mut activity_style = String::new();
    let mut activity_script = String::new();
    for (relative, kind) in [
        ("app/assets/stylesheets", "css"),
        ("app/javascript", "js"),
        ("vendor/javascript", "js"),
        ("rust/vendor/hotwired", "js"),
        ("rust/static", "native"),
        ("public", "public"),
    ] {
        let base = root.join(relative);
        println!("cargo:rerun-if-changed={}", base.display());
        for path in files(&base) {
            let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if kind == "native" && !matches!(extension, "css" | "js") {
                continue;
            }
            if kind != "public" && kind != "native" && extension != kind {
                continue;
            }
            if kind == "public" && path.parent() != Some(base.as_path()) {
                continue;
            }
            let name = path
                .strip_prefix(&base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let content = fs::read(&path).unwrap();
            let digest: String = Sha256::digest(&content)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let stem = name.strip_suffix(&format!(".{extension}")).unwrap_or(&name);
            let url = if kind == "public" {
                format!("/{name}")
            } else {
                format!("/assets/{stem}-{}.{extension}", &digest[..12])
            };
            let mime = match extension {
                "css" => "text/css; charset=utf-8",
                "js" => "text/javascript; charset=utf-8",
                "svg" => "image/svg+xml",
                "png" => "image/png",
                "ico" => "image/x-icon",
                "html" => "text/html; charset=utf-8",
                "txt" => "text/plain; charset=utf-8",
                _ => "application/octet-stream",
            };
            if kind == "css" {
                styles.push(url.clone());
            }
            if kind == "native" {
                if extension == "css" {
                    activity_style = url.clone();
                }
                if extension == "js" {
                    activity_script = url.clone();
                }
            }
            if kind == "js" {
                let logical = match stem {
                    "turbo.min" => "@hotwired/turbo-rails",
                    "stimulus.min" => "@hotwired/stimulus",
                    "stimulus-loading" => "@hotwired/stimulus-loading",
                    "controllers/index" => "controllers",
                    other => other,
                };
                imports.insert(logical.to_owned(), url.clone());
            }
            assets.push(format!(
                "({url:?}, {mime:?}, include_bytes!({:?}))",
                path.to_string_lossy()
            ));
        }
    }
    let importmap = serde_json::json!({"imports":imports}).to_string();
    let output = format!(
        "pub static ASSETS: &[(&str,&str,&[u8])] = &[{}];\npub static STYLES: &[&str] = &{:?};\npub const IMPORTMAP: &str = {:?};\npub const ACTIVITY_STYLE: &str = {:?};\npub const ACTIVITY_SCRIPT: &str = {:?};\n",
        assets.join(",\n"),
        styles,
        importmap,
        activity_style,
        activity_script
    );
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("assets.rs"),
        output,
    )
    .unwrap();
}
