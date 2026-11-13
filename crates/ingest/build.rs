use std::fmt::Write;
use std::path::Path;
use std::{env, fs};

fn main() {
    let recipes = Path::new(env!("CARGO_MANIFEST_DIR")).join("recipes");
    println!("cargo:rerun-if-changed={}", recipes.display());
    let mut files = Vec::new();
    for recipe in sorted(&recipes) {
        let gold = recipe.join("gold");
        println!("cargo:rerun-if-changed={}", gold.display());
        for split in sorted(&gold) {
            println!("cargo:rerun-if-changed={}", split.display());
            for label in sorted(&split) {
                if label
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    files.push((name(&recipe), name(&split), label));
                }
            }
        }
    }
    let mut code = String::from("pub(crate) static GOLD_FILES: &[GoldFile] = &[\n");
    for (recipe, split, label) in files {
        let text = label.with_extension("txt");
        writeln!(
            code,
            "    GoldFile {{ recipe: {recipe:?}, split: {split:?}, text: include_str!({:?}), label: include_str!({:?}) }},",
            text.display().to_string(),
            label.display().to_string(),
        )
        .expect("writing to a string");
    }
    code.push_str("];\n");
    let out = Path::new(&env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("gold.rs");
    fs::write(out, code).expect("the gold file list is written");
}

fn sorted(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    entries
}

fn name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .expect("a UTF-8 file name")
        .to_owned()
}
