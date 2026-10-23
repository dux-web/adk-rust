//! Temporary standalone Windows peer, linked with a static CRT for restricted-token diagnosis.

use std::ffi::c_void;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn GetLastError() -> u32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

fn main() {
    let mut observations = String::new();
    for (name, access) in [("query", 0x0008), ("duplicate", 0x0002), ("assign", 0x0001)] {
        let mut token = std::ptr::null_mut();
        let ok = unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut token) };
        let error = if ok == 0 { unsafe { GetLastError() } } else { 0 };
        observations.push_str(&format!("token {name}: ok={ok}, error={error}\n"));
        if !token.is_null() {
            unsafe { CloseHandle(token) };
        }
    }
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "echo child> child.txt"])
        .output();
    observations.push_str(&match output {
        Ok(output) => format!(
            "child status={:?}; stdout={:?}; stderr={:?}\n",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(error) => format!("child spawn error={error:?}; raw={:?}\n", error.raw_os_error()),
    });
    std::fs::write("peer-outcome.txt", observations).unwrap();
}
