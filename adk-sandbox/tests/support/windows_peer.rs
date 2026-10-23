//! Temporary standalone Windows peer, linked with a static CRT for restricted-token diagnosis.

use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

#[repr(C)]
struct StartupInfo {
    size: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    x_chars: u32,
    y_chars: u32,
    fill: u32,
    flags: u32,
    show: u16,
    reserved_size: u16,
    reserved_bytes: *mut u8,
    stdin: *mut c_void,
    stdout: *mut c_void,
    stderr: *mut c_void,
}

#[repr(C)]
struct ProcessInfo {
    process: *mut c_void,
    thread: *mut c_void,
    process_id: u32,
    thread_id: u32,
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn GetLastError() -> u32;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn CreateProcessW(
        application: *const u16,
        command: *mut u16,
        process_security: *const c_void,
        thread_security: *const c_void,
        inherit: i32,
        flags: u32,
        environment: *const c_void,
        directory: *const u16,
        startup: *const StartupInfo,
        process: *mut ProcessInfo,
    ) -> i32;
    fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
    fn GetExitCodeProcess(handle: *mut c_void, exit_code: *mut u32) -> i32;
}

fn direct_spawn(program: &Path) -> String {
    let application: Vec<u16> = program.as_os_str().encode_wide().chain(Some(0)).collect();
    let line = format!("\"{}\" /d /c echo direct> direct-child.txt", program.display());
    let mut command: Vec<u16> = OsStr::new(&line).encode_wide().chain(Some(0)).collect();
    let mut startup: StartupInfo = unsafe { std::mem::zeroed() };
    startup.size = std::mem::size_of::<StartupInfo>() as u32;
    let mut process: ProcessInfo = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            0,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    };
    if ok == 0 {
        return format!("CreateProcessW ok=0 error={}\n", unsafe { GetLastError() });
    }
    let wait = unsafe { WaitForSingleObject(process.process, 10000) };
    let mut code = 0;
    let queried = unsafe { GetExitCodeProcess(process.process, &mut code) };
    unsafe {
        CloseHandle(process.thread);
        CloseHandle(process.process);
    }
    format!("CreateProcessW ok=1 wait={wait} queried={queried} exit={code:#x}\n")
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
    let system = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
    for name in ["cmd.exe", "kernel32.dll", "ucrtbase.dll", "vcruntime140.dll"] {
        let path = system.join(name);
        let metadata = std::fs::metadata(&path).map(|metadata| metadata.len());
        let readable = std::fs::File::open(&path).map(|_| ());
        observations
            .push_str(&format!("system {name}: metadata={metadata:?}; read={readable:?}\n"));
    }
    let local = std::env::current_dir().unwrap().join("localcmd.exe");
    for program in [Path::new("cmd.exe"), &system.join("cmd.exe"), &local] {
        let output = std::process::Command::new(program)
            .args(["/d", "/c", "echo child> child.txt"])
            .output();
        observations.push_str(&format!("spawn {:?}: ", program));
        observations.push_str(&match output {
            Ok(output) => format!(
                "status={:?}; stdout={:?}; stderr={:?}\n",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
            Err(error) => format!("error={error:?}; raw={:?}\n", error.raw_os_error()),
        });
    }
    observations.push_str(&format!("direct system: {}", direct_spawn(&system.join("cmd.exe"))));
    observations.push_str(&format!("direct local: {}", direct_spawn(&local)));
    std::fs::write("peer-outcome.txt", observations).unwrap();
}
