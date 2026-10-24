#![cfg(test)]

use std::path::Path;

#[test]
fn window_code_has_no_unowned_launcher_or_recognized_wait() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut folders = ["app", "win", "core"].map(|area| root.join(area)).to_vec();
    let mut failures = Vec::new();
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).expect("source folders are readable") {
            let path = entry.expect("source entries are readable").path();
            if path.is_dir() {
                folders.push(path);
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("Rust source is UTF-8");
            if source.starts_with("#![cfg(test)]") {
                continue;
            }
            let path = path.strip_prefix(&root).expect("source stays under src");
            let name = path.to_string_lossy().replace('\\', "/");
            let worker = path.components().any(|part| part.as_os_str() == "work")
                || name.ends_with("/work.rs")
                || name.ends_with("_thread.rs");
            let mut test_item = None;
            let mut function = String::new();
            let mut header = String::new();
            for (index, line) in source.lines().enumerate() {
                let text = line.trim();
                let indent = line.len() - line.trim_start().len();
                if text == "#[cfg(test)]" {
                    test_item = Some(indent);
                    continue;
                }
                if let Some(start) = test_item {
                    if indent == start && (text == "}" || text.ends_with(';')) {
                        test_item = None;
                    }
                    continue;
                }
                if text.starts_with("//") {
                    continue;
                }
                if let Some((_, after)) = text.split_once("fn ") {
                    function = after.split(['(', '<']).next().unwrap_or("").to_owned();
                    header.clear();
                }
                if !header.contains('{') {
                    header.push_str(text);
                }
                let code: String = text
                    .split("//")
                    .next()
                    .unwrap_or("")
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .collect();
                let launcher = code.contains("thread::spawn(") || code.contains("thread::Builder");
                let exception = exception(&name, &function);
                let owned_launcher = name == "win/work.rs"
                    || name.starts_with("win/work/")
                    || name == "win/shortcut.rs" && function == "start"
                    || name == "app/mod.rs" && function == "install_panic_hook";
                let waiting = [
                    "fs::read",
                    "fs::write(",
                    "fs::metadata(",
                    "fs::symlink_metadata(",
                    "fs::canonicalize(",
                    "fs::create_dir",
                    "fs::remove_",
                    "fs::rename(",
                    "File::open(",
                    "File::create(",
                    "OpenOptions::new(",
                    ".exists()",
                    ".is_dir()",
                    ".is_file()",
                    ".metadata()",
                    ".symlink_metadata()",
                    ".canonicalize()",
                    ".read_dir()",
                    ".join()",
                    ".recv()",
                    "thread::sleep(",
                    "Command::new(",
                    ".wait()",
                    "ShellExecuteW(",
                    "ShellExecuteExW(",
                    "SHOpenFolderAndSelectItems(",
                    "GetVolumeInformationW(",
                    "RegGetValueW(",
                    "RegSetKeyValueW(",
                    "RegDeleteKeyValueW(",
                    "RegDeleteTreeW(",
                    "OpenClipboard(",
                    "Shell_NotifyIconW(",
                    "folder_is_missing(",
                    "Queue::new(",
                    "WinHttpSendRequest(",
                    "WinHttpReceiveResponse(",
                    "WinHttpReadData(",
                    "update::latest_release(",
                    "update::read(",
                    "update::sha256(",
                ]
                .iter()
                .any(|call| code.contains(call));
                let gated = name.starts_with("win/") && header.contains("OffThread");
                if launcher && !owned_launcher
                    || waiting && !exception && (!worker || name.starts_with("core/")) && !gated
                {
                    failures.push(format!("{name}:{} ({function}): {text}", index + 1));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "unowned UI work:\n{}",
        failures.join("\n")
    );
}

fn exception(file: &str, function: &str) -> bool {
    match file {
        // No HWND exists during startup/reset; panic reporting cannot dispatch
        // into half-mutated window state. Logging otherwise runs on workers.
        "app/mod.rs" => matches!(
            function,
            "run"
                | "reset_and_start_again"
                | "instance_suffix"
                | "runs_another_program"
                | "install_panic_hook"
                | "write_log"
        ),
        "app/include.rs" => function == "save_as_administrator",
        // The low-level keyboard hook inherently owns a native message loop.
        "win/shortcut.rs" => function == "run_hook",
        // Private native helpers are reached through the worker-only Session,
        // token-gated registry functions, or startup registry migration.
        "win/shell.rs" => matches!(
            function,
            "execute"
                | "show_in_folder"
                | "set_string_in"
                | "get_value"
                | "delete_value"
                | "remove_search_with"
        ),
        _ => false,
    }
}
