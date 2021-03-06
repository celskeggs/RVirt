use riscv_decode::Instruction;
use crate::context::{Context, CONTEXT};
use crate::{pfault, pmap, riscv, sum};

#[allow(unused)]
pub mod constants {
    pub const TVEC_MODE: u64 = 0x3;
    pub const TVEC_BASE: u64 = !TVEC_MODE;

    pub const STATUS_UIE: u64 = 1 << 0;
    pub const STATUS_SIE: u64 = 1 << 1;
    pub const STATUS_UPIE: u64 = 1 << 4;
    pub const STATUS_SPIE: u64 = 1 << 5;
    pub const STATUS_SPP: u64 = 1 << 8;
    pub const STATUS_FS: u64 = 3 << 13;
    pub const STATUS_XS: u64 = 3 << 15;
    pub const STATUS_SUM: u64 = 1 << 18;
    pub const STATUS_MXR: u64 = 1 << 19;
    pub const STATUS_SD: u64 = 1 << 63;

    pub const STATUS_MPP_M: u64 = 3 << 11;
    pub const STATUS_MPP_S: u64 = 1 << 11;
    pub const STATUS_MPP_U: u64 = 0 << 11;

    // Mask of writable bits in sstatus.
    pub const SSTATUS_WRITABLE_MASK: u64 =
        STATUS_MXR |
        STATUS_SUM |
        STATUS_FS |
        STATUS_SPP |
        STATUS_SPIE |
        STATUS_SIE;
    pub const SSTATUS_DYNAMIC_MASK: u64 = STATUS_SD | STATUS_FS;

    pub const IP_SSIP: u64 = 1 << 1;
    pub const IP_STIP: u64 = 1 << 5;
    pub const IP_SEIP: u64 = 1 << 9;

    pub const IE_SSIE: u64 = 1 << 1;
    pub const IE_STIE: u64 = 1 << 5;
    pub const IE_SEIE: u64 = 1 << 9;

    pub const SATP_MODE: u64 = 0xf << 60;
    pub const SATP_ASID: u64 = 0xffff << 44;
    pub const SATP_PPN: u64 = 0xfff_ffffffff;

    pub const SSTACK_BASE: u64 = 0xffffffffc0a00000 - 32*8;

    pub const SCAUSE_INSN_MISALIGNED: u64 = 0;
    pub const SCAUSE_INSN_ACCESS_FAULT: u64 = 1;
    pub const SCAUSE_ILLEGAL_INSN: u64 = 2;
    pub const SCAUSE_BREAKPOINT: u64 = 3;
    pub const SCAUSE_LOAD_ACCESS_FAULT: u64 = 5;
    pub const SCAUSE_ATOMIC_MISALIGNED: u64 = 6;
    pub const SCAUSE_STORE_ACCESS_FAULT: u64 = 7;
    pub const SCAUSE_ENV_CALL: u64 = 8;
    pub const SCAUSE_INSN_PAGE_FAULT: u64 = 12;
    pub const SCAUSE_LOAD_PAGE_FAULT: u64 = 13;
    pub const SCAUSE_STORE_PAGE_FAULT: u64 = 15;

    pub const CAUSE_STRINGS: [&str; 16] = [
        "Instruction address misaligned",
        "Instruction access fault",
        "Illegal instruction",
        "Breakpoint",
        "Load address misaligned",
        "Load access fault",
        "Store/AMO address misaligned",
        "Store/AMO access fault",
        "Environment call from U-mode",
        "Environment call from S-mode",
        "Reserved (10)",
        "Environment call from M-mode",
        "Instruction page fault",
        "Load page fault",
        "Reserved (13)",
        "Store/AMO page fault"
    ];

    pub fn cause_to_str(cause: u64) -> &'static str {
        if (cause as i64) < 0 {
            "Interrupt"
        } else if cause >= 16 {
            "Reserved (>=16)"
        } else {
            CAUSE_STRINGS[cause as usize]
        }
    }
}
use self::constants::*;

pub trait U64Bits {
    fn get(&self, mask: Self) -> bool;
    fn set(&mut self, mask: Self, value: bool);
}
impl U64Bits for u64 {
    #[inline(always)]
    fn get(&self, mask: Self) -> bool {
        *self & mask != 0
    }
    #[inline(always)]
    fn set(&mut self, mask: Self, value: bool) {
        if value {
            *self |= mask;
        } else {
            *self &= !mask;
        }
    }
}

#[naked]
#[no_mangle]
pub unsafe fn strap_entry() -> ! {
    asm!(".align 4
          csrw 0x140, sp      // Save stack pointer in sscratch
          li sp, $0           // Set stack pointer

          // Save registers
          sd ra, 1*8(sp)
          sd gp, 3*8(sp)
          sd tp, 4*8(sp)
          sd t0, 5*8(sp)
          sd t1, 6*8(sp)
          sd t2, 7*8(sp)
          sd s0, 8*8(sp)
          sd s1, 9*8(sp)
          sd a0, 10*8(sp)
          sd a1, 11*8(sp)
          sd a2, 12*8(sp)
          sd a3, 13*8(sp)
          sd a4, 14*8(sp)
          sd a5, 15*8(sp)
          sd a6, 16*8(sp)
          sd a7, 17*8(sp)
          sd s2, 18*8(sp)
          sd s3, 19*8(sp)
          sd s4, 20*8(sp)
          sd s5, 21*8(sp)
          sd s6, 22*8(sp)
          sd s7, 23*8(sp)
          sd s8, 24*8(sp)
          sd s9, 25*8(sp)
          sd s10, 26*8(sp)
          sd s11, 27*8(sp)
          sd t3, 28*8(sp)
          sd t4, 29*8(sp)
          sd t5, 30*8(sp)
          sd t6, 31*8(sp)

          jal ra, strap       // Call `strap`
          li sp, $0           // Reset stack pointer, just to be safe

          // Restore registers
          ld ra, 1*8(sp)
          ld gp, 3*8(sp)
          ld tp, 4*8(sp)
          ld t0, 5*8(sp)
          ld t1, 6*8(sp)
          ld t2, 7*8(sp)
          ld s0, 8*8(sp)
          ld s1, 9*8(sp)
          ld a0, 10*8(sp)
          ld a1, 11*8(sp)
          ld a2, 12*8(sp)
          ld a3, 13*8(sp)
          ld a4, 14*8(sp)
          ld a5, 15*8(sp)
          ld a6, 16*8(sp)
          ld a7, 17*8(sp)
          ld s2, 18*8(sp)
          ld s3, 19*8(sp)
          ld s4, 20*8(sp)
          ld s5, 21*8(sp)
          ld s6, 22*8(sp)
          ld s7, 23*8(sp)
          ld s8, 24*8(sp)
          ld s9, 25*8(sp)
          ld s10, 26*8(sp)
          ld s11, 27*8(sp)
          ld t3, 28*8(sp)
          ld t4, 29*8(sp)
          ld t5, 30*8(sp)
          ld t6, 31*8(sp)

          // Restore stack pointer and return
          csrr sp, 0x140
          sret" :: "i"(SSTACK_BASE) : "memory" : "volatile");

    unreachable!()
}

#[no_mangle]
pub fn strap() {
    let cause = csrr!(scause);
    let status = csrr!(sstatus);

    if status.get(STATUS_SPP) {
        println!("Trap from within hypervisor?!");
        println!("sepc = {:#x}", csrr!(sepc));
        println!("stval = {:#x}", csrr!(stval));
        println!("cause = {}", cause);

        // No other threads could be accessing CONTEXT here, and even if we interrupted a critical
        // section, we're about to crash anyway so it doesn't matter that much.
        unsafe { CONTEXT.force_unlock() }
        let mut state = CONTEXT.lock();
        let mut state = (&mut *state).as_mut().unwrap();

        println!("reg ra = {:#x}", get_register(&mut state, 1));
        println!("reg sp = {:#x}", get_register(&mut state, 2));
        for i in 3..32 {
            println!("reg x{} = {:#x}", i, get_register(&mut state, i));
        }

        loop {}
    }

    let mut state = CONTEXT.lock();
    let mut state = (&mut *state).as_mut().unwrap();

    // For the processor to have generated a load/store page fault or an illegal instruction fault,
    // the processor must have been able to load the relevant instruction (or else an access fault
    // or instruction page fault would have been triggered). Thus, it is safe to access memory
    // pointed to by `sepc`.
    let instruction = match cause {
        SCAUSE_LOAD_PAGE_FAULT |
        SCAUSE_STORE_PAGE_FAULT |
        SCAUSE_ILLEGAL_INSN => unsafe {
            Some(load_instruction_at_address(&mut state, csrr!(sepc)))
        }
        _ => None,
    };

    if (cause as isize) < 0 {
        handle_interrupt(&mut state, cause);
        maybe_forward_interrupt(&mut state, csrr!(sepc));
    } else if cause == SCAUSE_INSN_PAGE_FAULT || cause == SCAUSE_LOAD_PAGE_FAULT || cause == SCAUSE_STORE_PAGE_FAULT {
        let pc = csrr!(sepc);
        if pfault::handle_page_fault(&mut state, cause, instruction.map(|i|i.0)) {
            maybe_forward_interrupt(&mut state, pc);
        } else {
            forward_exception(&mut state, cause, pc);
        }
    } else if cause == SCAUSE_ILLEGAL_INSN && state.smode {
        let pc = csrr!(sepc);
        let (instruction, len) = instruction.unwrap();
        let mut advance_pc = true;
        match riscv_decode::decode(instruction).ok() {
            Some(Instruction::Sret) => {
                if !state.csrs.sstatus.get(STATUS_SIE) && state.csrs.sstatus.get(STATUS_SPIE) {
                    state.no_interrupt = false;
                }
                state.csrs.pop_sie();
                state.smode = state.csrs.sstatus.get(STATUS_SPP);
                state.csrs.sstatus.set(STATUS_SPP, false);
                riscv::set_sepc(state.csrs.sepc);
                advance_pc = false;

                if !state.smode {
                    state.no_interrupt = false;
                }
            }
            Some(Instruction::SfenceVma(rtype)) => pmap::handle_sfence_vma(&mut state, rtype),
            Some(Instruction::Csrrw(i)) => if let Some(prev) = state.get_csr(i.csr()) {
                let value = get_register(state, i.rs1());
                state.set_csr(i.csr(), value);
                set_register(state, i.rd(), prev);
            }
            Some(Instruction::Csrrs(i)) => if let Some(prev) = state.get_csr(i.csr()) {
                let mask = get_register(state, i.rs1());
                if mask != 0 {
                    state.set_csr(i.csr(), prev | mask);
                }
                set_register(state, i.rd(), prev);
            }
            Some(Instruction::Csrrc(i)) => if let Some(prev) = state.get_csr(i.csr()) {
                let mask = get_register(state, i.rs1());
                if mask != 0 {
                    state.set_csr(i.csr(), prev & !mask);
                }
                set_register(state, i.rd(), prev);
            }
            Some(Instruction::Csrrwi(i)) => if let Some(prev) = state.get_csr(i.csr()) {
                state.set_csr(i.csr(), i.zimm() as u64);
                set_register(state, i.rd(), prev);
            }
            Some(Instruction::Csrrsi(i)) => if let Some(prev) = state.get_csr(i.csr()) {
                let mask = i.zimm() as u64;
                if mask != 0 {
                    state.set_csr(i.csr(), prev | mask);
                }
                set_register(state, i.rd(), prev);
            }
            Some(Instruction::Csrrci(i)) => if let Some(prev) = state.get_csr(i.csr()) {
                let mask = i.zimm() as u64;
                if mask != 0 {
                    state.set_csr(i.csr(), prev & !mask);
                }
                set_register(state, i.rd(), prev);
            }
            Some(Instruction::Wfi) => riscv::wfi(),
            Some(decoded) => {
                println!("Unrecognized instruction! {:?} @ pc={:#x}", decoded, pc);
                forward_exception(&mut state, cause, pc);
                advance_pc = false;
            }
            None => {
                println!("Unrecognized instruction {:#x} @ pc={:#x}", instruction, pc);
                forward_exception(&mut state, cause, pc);
                advance_pc = false;
            }
        }

        if advance_pc {
            riscv::set_sepc(pc + len);
        }
        maybe_forward_interrupt(&mut state, csrr!(sepc));
    } else if cause == SCAUSE_ENV_CALL && state.smode {
        match get_register(state, 17) {
            0 => {
                state.csrs.sip.set(IP_STIP, false);
                state.csrs.mtimecmp = get_register(state, 10);
                state.host_clint.set_mtimecmp(state.csrs.mtimecmp);
            }
            1 => {
                let value = get_register(state, 10) as u8;
                state.uart.output_byte(value)
            }
            5 => riscv::fence_i(),
            6 | 7 => {
                // Current versions of the Linux kernel pass wrong arguments to these SBI calls. As
                // a result, this function ignores the arguments and just does a global fence. This
                // will eventually be fixed by https://patchwork.kernel.org/patch/10872353.
                pmap::flush_shadow_page_table(&mut state.shadow_page_tables);
            }
            i => {
                println!("Got ecall from guest function={}!", i);
                loop {}
            }
        }
        riscv::set_sepc(csrr!(sepc) + 4);
    } else {
        if cause != SCAUSE_ENV_CALL { // no need to print anything for guest syscalls...
            println!("Forward exception (cause = {}, smode={})!", cause, state.smode);
        } else {
            // println!("system call: {}({:#x}, {:#x}, {:#x}, {:#x})",
            //          syscall_name(get_register(state, 17)),
            //          get_register(state, 10), get_register(state, 11),
            //          get_register(state, 12), get_register(state, 13)
            // );
            // if syscall_name(get_register(state, 17)) == "write" {
            //     let fd = get_register(state, 10);
            //     let ptr = get_register(state, 11);
            //     let len = get_register(state, 12);
            //     if fd == 1 {
            //         print!("data = ");
            //         for i in 0..len {
            //             print::guest_putchar(*((ptr + i) as *const u8));
            //         }
            //     }
            // }
        }
        forward_exception(&mut state, cause, csrr!(sepc));
    }

    state.shadow_page_tables.install_root(state.shadow());
}

fn handle_interrupt(state: &mut Context, cause: u64) {
    let interrupt = cause & 0xff;
    match interrupt {
        0x1 => {
            // Software interrupt. M-mode code actually uses these to single timer interrupts
            // because disarming a software interrupt doesn't require an SBI call but disarming
            // timer interrupts might. In more detail, on some hardware `sip.stip` will not be
            // writable while `sip.ssip` will be.
            riscv::clear_sip(1 << interrupt);
            assert_eq!(csrr!(sip) & (1 << interrupt), 0);

            let time = state.host_clint.get_mtime();
            crate::context::Uart::timer(state, time);
            if state.csrs.mtimecmp <= time {
                state.csrs.sip |= IP_STIP;
                state.no_interrupt = false;
            }

            let mut next = 0xffffffff;
            if state.uart.next_interrupt_time > time {
                next = next.min(state.uart.next_interrupt_time);
            }
            if state.csrs.mtimecmp > time {
                next = next.min(state.csrs.mtimecmp);
            }
            if next < 0xffffffff {
                state.host_clint.set_mtimecmp(next);
            }
        }
        0x5 => {
            // Supervisor timer interrupt. This is unreachable because the M-mode code will always
            // generate supervisor *software* interrupts instead. See the case above for more
            // details.
            unreachable!();
        }
        0x9 => {
            // External
            let host_irq = state.host_plic.claim_and_clear();
            let guest_irq = state.irq_map[host_irq as usize];
            if guest_irq != 0 {
                state.plic.set_pending(guest_irq as u32, true);

                // Guest might have masked out this interrupt
                if state.plic.interrupt_pending() {
                    state.no_interrupt = false;
                    state.csrs.sip |= IP_SEIP;
                } else {
                    assert_eq!(state.csrs.sip & IP_SEIP, 0);
                }
            }

        }
        i => {
            println!("Got interrupt #{}", i);
            unreachable!()
        }
    }
}

fn maybe_forward_interrupt(state: &mut Context, sepc: u64) {
    if state.no_interrupt {
        return;
    }

    if !state.csrs.sip.get(IP_SEIP) && state.plic.interrupt_pending() {
        state.csrs.sip.set(IP_SEIP, true);
    }

    if (!state.smode || state.csrs.sstatus.get(STATUS_SIE)) && (state.csrs.sie & state.csrs.sip != 0) {
        let cause = if state.csrs.sip.get(IP_SEIP) {
            9
        } else if state.csrs.sip.get(IP_STIP) {
            5
        } else if state.csrs.sip.get(IP_SSIP) {
            1
        } else {
            unreachable!()
        };

        // println!("||> Forwarding timer interrupt! (state.smode={}, sepc={:#x})", state.smode, sepc);
        // forward interrupt
        state.csrs.push_sie();
        state.csrs.sepc = sepc;
        state.csrs.scause = (1 << 63) | cause;
        state.csrs.sstatus.set(STATUS_SPP, state.smode);
        state.csrs.stval = 0;
        state.smode = true;

        match state.csrs.stvec & TVEC_MODE {
            0 => riscv::set_sepc(state.csrs.stvec & TVEC_BASE),
            1 => riscv::set_sepc((state.csrs.stvec & TVEC_BASE) + 4 * cause),
            _ => unreachable!(),
        }
    } else {
        state.no_interrupt = true;
    }
}

fn forward_exception(state: &mut Context, cause: u64, sepc: u64) {
    // println!("||> Forward exception sepc={:#x}", sepc);
    state.csrs.push_sie();
    state.csrs.sepc = sepc;
    state.csrs.scause = cause;
    state.csrs.sstatus.set(STATUS_SPP, state.smode);
    state.csrs.stval = csrr!(stval);
    state.smode = true;
    riscv::set_sepc(state.csrs.stvec & TVEC_BASE);
}

pub fn set_register(state: &mut Context, reg: u32, value: u64) {
    match reg {
        0 => {},
        1 | 3..=31 => state.saved_registers[reg as u64 * 8] = value,
        2 => riscv::set_sscratch(value),
        _ => unreachable!(),
    }
}
pub fn get_register(state: &mut Context, reg: u32) -> u64 {
    match reg {
        0 => 0,
        1 | 3..=31 => state.saved_registers[reg as u64 * 8],
        2 => csrr!(sscratch),
        _ => unreachable!(),
    }
}

pub unsafe fn load_instruction_at_address(_state: &mut Context, guest_va: u64) -> (u32, u64) {
    let pc_ptr = guest_va as *const u16;
    sum::access_user_memory(||{
        let il: u16 = *pc_ptr;
        match riscv_decode::instruction_length(il) {
            2 => (il as u32, 2),
            4 => (il as u32 | ((*pc_ptr.offset(1) as u32) << 16), 4),
            _ => unreachable!(),
        }
    })
}
