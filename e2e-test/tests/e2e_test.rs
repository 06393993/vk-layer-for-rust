// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{env, path::PathBuf};

use assert_cmd::Command;
use escargot::format::Message;

#[test]
fn test_vulkaninfo_without_layers() {
    let mut cmd = Command::new("vulkaninfo");
    cmd.arg("--summary");

    cmd.assert().success();
}

#[test]
fn test_vulkaninfo_with_example_layers() {
    let mut workspace_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR")
            .expect("cargo must set the CARGO_MANIFEST_DIR environment variable"),
    );
    assert!(workspace_dir.is_absolute());
    assert!(workspace_dir.pop());
    let layer_project_dir = workspace_dir.join("examples").join("hello-world");

    let build_messages = escargot::CargoBuild::new()
        .current_release()
        .current_target()
        .all_features()
        .manifest_path({
            let mut path = layer_project_dir.clone();
            path.push("Cargo.toml");
            path
        })
        .exec()
        .expect("build should succeed");
    let layer_library_path = build_messages
        .flat_map(|build_message| {
            let build_message = build_message.expect("the cargo command must succeed");
            let build_message = build_message
                .decode()
                .expect("the cargo message must be decoded successfully");
            let artifact = match build_message {
                Message::CompilerArtifact(artifact) => artifact,
                _ => return vec![],
            };
            artifact
                .filenames
                .into_iter()
                .map(|path| path.into_owned())
                .collect::<Vec<_>>()
        })
        .find_map(|artifact_file_path| {
            let extension = artifact_file_path.extension()?;
            let extension = extension.to_str()?.to_lowercase();
            if !(extension == "dll" || extension == "so") {
                return None;
            }
            let file_name = artifact_file_path.file_name()?;
            let file_name = file_name.to_str()?;
            if !(file_name.starts_with("VkLayer") || file_name.starts_with("libVkLayer")) {
                return None;
            }
            Some(artifact_file_path)
        })
        .expect("must build the layer library");

    let layer_library_dir = layer_library_path
        .parent()
        .expect("the layer library must have a parent");
    let manifest_file_name = "rust_example_layer.json";
    cfg_if::cfg_if! {
        if #[cfg(windows)] {
            let platform = "windows";
        } else if #[cfg(target_os = "linux")] {
            let platform = "linux";
        } else {
            compile_error!("Unsupported platform")
        }
    }
    std::fs::copy(
        layer_project_dir
            .join("manifest")
            .join(platform)
            .join(manifest_file_name),
        layer_library_dir.join(manifest_file_name),
    )
    .expect("copy must complete successfully");

    let mut cmd = Command::new("vulkaninfo");
    cmd.arg("--summary")
        .env("VK_ADD_LAYER_PATH", layer_library_dir)
        .env(
            "VK_LOADER_LAYERS_ENABLE",
            "VK_LAYER_VENDOR_rust_example_hello_world",
        )
        .env(
            "VK_LOADER_LAYERS_ALLOW",
            "VK_LAYER_VENDOR_rust_example_hello_world",
        );

    cmd.assert().success().stdout(predicates::str::contains(
        "Hello from the Rust Vulkan layer!",
    ));
}
