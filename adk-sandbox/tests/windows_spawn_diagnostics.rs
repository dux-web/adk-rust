//! Temporary fork-only diagnosis of nested shell commands under the Windows enforcer.
#![cfg(all(windows, feature = "sandbox-windows"))]

use std::ffi::OsString;
use std::time::Duration;

use adk_sandbox::sandbox::SandboxPolicyBuilder;
use adk_sandbox::sandbox::windows::WindowsEnforcer;

#[test]
fn nested_shell_commands() {
    for (name, script) in [
        ("caret", "cmd /c echo child^> child.txt 2> spawn-err.txt"),
        ("plain", "cmd /c echo child> child.txt 2> spawn-err.txt"),
        ("batch", "cmd /c child.cmd 2> spawn-err.txt"),
        ("absolute_batch", "%SystemRoot%\\System32\\cmd.exe /c child.cmd 2> spawn-err.txt"),
        (
            "start_caret",
            "start /b cmd /c for /l %i in (1,1,1000000) do @echo x^>^> tick.txt 2> start-err.txt & for /l %j in (1,1,1000000) do @rem",
        ),
        (
            "start_batch",
            "start /b cmd /c writer.cmd 2> start-err.txt & for /l %j in (1,1,1000000) do @rem",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(root.path()).unwrap();
        std::fs::write(workspace.join("child.cmd"), "@echo off\r\necho child> child.txt\r\n")
            .unwrap();
        std::fs::write(
            workspace.join("writer.cmd"),
            "@echo off\r\nfor /l %%i in (1,1,1000000) do @echo x>> tick.txt\r\n",
        )
        .unwrap();
        let policy = SandboxPolicyBuilder::new().allow_read_write(&workspace).build();
        let result = WindowsEnforcer::new().run_confined(
            &policy,
            "cmd.exe".as_ref(),
            &[OsString::from("/c"), OsString::from(script)],
            &workspace,
            Some(Duration::from_secs(20)),
        );
        let files: Vec<_> = std::fs::read_dir(&workspace)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let len = std::fs::metadata(&path).unwrap().len();
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    len,
                    text.chars().take(300).collect::<String>(),
                )
            })
            .collect();
        println!("PROBE {name}: {result:?}; FILES {files:?}");
        assert!(result.is_ok(), "the enforcer must launch the probe shell");
    }
}
