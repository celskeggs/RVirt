use byteorder::{NativeEndian, ByteOrder};
use riscv_decode::Instruction;
use crate::context::Context;
use crate::memory_region::MemoryRegion;
use crate::{pmap, riscv, trap};

pub const MAX_QUEUES: usize = 4;
pub const MAX_DEVICES: usize = 4;

#[derive(Copy, Clone)]
pub struct Queue {
    /// Address guest thinks queue is mapped at
    guest_pa: u64,
    /// Address queue is actually mapped at
    host_pa: u64,
    /// Number of entries in queue
    size: u64,
}

pub struct Device {
    /// Virtual Queue Index, offset=0x30
    queue_sel: u32,
    queues: [Queue; MAX_QUEUES],
    device_registers: MemoryRegion<u32>,
}
impl Device {
    pub unsafe fn new(host_base_address: u64) -> Self {
        Self {
            queue_sel: 0,
            queues: [Queue {guest_pa: 0, host_pa: 0, size: 0}; MAX_QUEUES],
            device_registers: MemoryRegion::with_base_address(pmap::pa2va(host_base_address), 0, 0x1000),
        }
    }
}

#[inline(always)]
pub fn is_device_access(state: &mut Context, guest_pa: u64) -> bool {
    guest_pa >= 0x10001000 && guest_pa < 0x10001000 + 0x1000 * state.virtio.devices.len() as u64
}

pub fn handle_device_access(state: &mut Context, guest_pa: u64, instruction: u32) -> bool {
    let device = ((guest_pa - 0x10001000) / 0x1000) as usize;
    let offset = guest_pa & 0xfff;

    let mut current = state.virtio.devices[device].device_registers[offset & !0x3];
    if offset == 0x10 {
        current = current & !(1 << 28); // No VIRTIO_F_INDIRECT_DESC
    } else if offset == 0x34 {
        current = current.min(256); // ensure queues take up at most one page
    }

    match riscv_decode::decode(instruction).ok() {
        Some(Instruction::Lw(i)) => {
            trap::set_register(state, i.rd(), current as u64)
        }
        Some(Instruction::Lb(i)) => {
            assert!(offset >= 0x100);
            let value = (current >> (8*(offset & 0x3))) & 0xff;
            trap::set_register(state, i.rd(), value as u64)
        }
        Some(Instruction::Sw(i)) => {
            let mut value = trap::get_register(state, i.rs2()) as u32;
            if offset == 0x30 { // QueueSel
                assert!(value < 4);
                state.virtio.devices[device].queue_sel = value;
            } else if offset == 0x38 { // QueueNum
                let queue_sel = state.virtio.devices[device].queue_sel as usize;
                let queue = &mut state.virtio.devices[device].queues[queue_sel];
                queue.size = value as u64;

                // Linux never changes queue sizes, so this isn't supported.
                assert_eq!(queue.host_pa, 0);
            } else if offset == 0x40 { // QueuePFN
                let queue_sel = state.virtio.devices[device].queue_sel as usize;
                let queue = &mut state.virtio.devices[device].queues[queue_sel];

                // Linux never releases queues, so this is currently unimplemented.
                assert_eq!(queue.host_pa, 0);

                if value != 0 {
                    queue.guest_pa = (value as u64) << 12;
                    value += (state.guest_shift >> 12) as u32;
                    queue.host_pa = (value as u64) << 12;
                } else {
                    unimplemented!();
                }

                // Sad, but necessary because we don't know all the places this page is mapped.
                pmap::flush_shadow_page_table(&mut state.shadow_page_tables);

                state.virtio.queue_guest_pages.push(queue.guest_pa);
                for i in 0..queue.size {
                    let value = &mut state.guest_memory[queue.guest_pa + i * 16];
                    *value = (*value).wrapping_add(state.guest_shift);
                }
            }
            state.virtio.devices[device].device_registers[offset] = value;
        }
        Some(instr) => {
            println!("VIRTIO: Instruction {:?} used to target addr {:#x} from pc {:#x}", instr, guest_pa, csrr!(sepc));
            loop {}
        }
        None => {
            println!("Unrecognized instruction targetting VIRTIO {:#x} at {:#x}!", instruction, csrr!(sepc));
            loop {}
        }
    }
    riscv::set_sepc(csrr!(sepc) + riscv_decode::instruction_length(instruction as u16) as u64);
    true
}

pub fn is_queue_access(state: &mut Context, guest_page: u64) -> bool {
    for i in 0..state.virtio.queue_guest_pages.len() {
        if state.virtio.queue_guest_pages[i] == guest_page {
            return true;
        }
    }
    false
}

pub fn handle_queue_access(state: &mut Context, guest_pa: u64, host_pa: u64, instruction: u32) -> bool {
    let mut hit_queue = false;
    for d in &state.virtio.devices {
        for q in &d.queues {
            if guest_pa >= q.guest_pa && guest_pa < q.guest_pa + q.size * 16 && guest_pa & 0xf < 8 {
                hit_queue = true;
            }
        }
    }

    let decoded = riscv_decode::decode(instruction);
    if let Err(err) = decoded {
        println!("Unrecognized instruction targetting VQUEUE {:#x} at {:#x} (error: {:?})!",
                 instruction, csrr!(sepc), err);
        loop {}
    }

    if hit_queue {
        match decoded.unwrap() {
            Instruction::Ld(i) => {
                trap::set_register(state, i.rd(), state.guest_memory[guest_pa].wrapping_sub(state.guest_shift));
            }
            Instruction::Sd(i) => {
                let value = trap::get_register(state, i.rs2());
                if value == 0 {
                    state.guest_memory[guest_pa] = 0;
                } else if state.guest_memory.in_region(value) {
                    state.guest_memory[guest_pa] = value.wrapping_add(state.guest_shift);
                } else {
                    loop {}
                }
            }
            instr => {
                println!("VQUEUE: Instruction {:?} used to target addr {:#x} from pc {:#x}",
                         instr, host_pa, csrr!(sepc));
                loop {}
            }
        }
    } else {
        let index = guest_pa & !0x7;
        let offset = (guest_pa % 8) as usize;
        let mut current = state.guest_memory[index].to_ne_bytes();
        match decoded.as_ref().unwrap() {
            Instruction::Ld(i) => trap::set_register(state, i.rd(), u64::from_ne_bytes(current)),
            Instruction::Lwu(i) => trap::set_register(state, i.rd(), NativeEndian::read_u32(&current[offset..]) as u64),
            Instruction::Lhu(i) => trap::set_register(state, i.rd(), NativeEndian::read_u16(&current[offset..]) as u64),
            Instruction::Lbu(i) => trap::set_register(state, i.rd(), current[offset] as u64),
            Instruction::Lw(i) => trap::set_register(state, i.rd(), NativeEndian::read_i32(&current[offset..]) as i64 as u64),
            Instruction::Lh(i) => trap::set_register(state, i.rd(), NativeEndian::read_i16(&current[offset..]) as i64 as u64),
            Instruction::Lb(i) => trap::set_register(state, i.rd(), current[offset] as i8 as i64 as u64),
            Instruction::Sd(i) => state.guest_memory[index] = trap::get_register(state, i.rs2()),
            Instruction::Sw(i) => {
                NativeEndian::write_u32(&mut current[offset..], trap::get_register(state, i.rs2()) as u32);
                state.guest_memory[index] = u64::from_ne_bytes(current);
            }
            Instruction::Sh(i) => {
                NativeEndian::write_u16(&mut current[offset..], trap::get_register(state, i.rs2()) as u16);
                state.guest_memory[index] = u64::from_ne_bytes(current);
            }
            Instruction::Sb(i) => {
                current[offset] = trap::get_register(state, i.rs2()) as u8;
                state.guest_memory[index] = u64::from_ne_bytes(current);
            }
            instr => {
                println!("VQUEUE: Instruction {:?} used to target addr {:#x} from pc {:#x}",
                         instr, host_pa, csrr!(sepc));
                loop {}
            }
        }
    }

    riscv::set_sepc(csrr!(sepc) + riscv_decode::instruction_length(instruction as u16) as u64);
    true
}
