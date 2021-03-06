use arrayvec::{ArrayString, ArrayVec};
use core::{mem, ptr};

const FDT_BEGIN_NODE: u32 = 0x01000000;
const FDT_END_NODE: u32 = 0x02000000;
const FDT_PROP: u32 = 0x03000000;
const FDT_NOP: u32 = 0x04000000;
const FDT_END: u32 = 0x09000000;

#[derive(Default)]
struct AddressMap(ArrayVec<[ArrayString<[u8; 16]>; Self::MAX_LEN]>);
impl AddressMap {
    const MAX_LEN: usize = 16;
    fn index_of(&mut self, value: &str) -> usize {
        for i in 0..self.0.len() {
            if value == &self.0[i] {
                return i;
            }
        }

        let array_string = ArrayString::from(value).unwrap();
        self.0.push(array_string);
        self.0.len() - 1
    }
}


#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UartType {
    Ns16550a,
    SiFive,
}

#[derive(Clone, Debug)]
pub struct Device {
    pub base_address: u64,
    pub size: u64,
    pub irq: u64,
}

#[derive(Clone, Debug)]
pub struct Hart {
    pub hartid: u64,
    pub plic_context: u64,
}

#[derive(Clone, Debug, Default)]
pub struct MachineMeta {
    pub physical_memory_offset: u64,
    pub physical_memory_size: u64,

    pub harts: ArrayVec<[Hart; 16]>,

    pub uart_type: Option<UartType>,
    pub uart_address: u64,

    pub plic_address: u64,
    pub clint_address: u64,

    pub virtio: ArrayVec<[Device; 16]>,

    pub initrd_start: u64,
    pub initrd_end: u64,
}

#[repr(C)]
pub struct Fdt {
    magic: u32,
    total_size: u32,
    off_dt_struct: u32,
    off_dt_strings: u32,
    off_mem_rsvmap: u32,
    version: u32,
    last_comp_version: u32,
    boot_cpuid_phys: u32,
    size_dt_strings: u32,
    size_dt_struct: u32,
}
#[allow(unused)]
impl Fdt {
    pub unsafe fn new(addr: u64) -> &'static Self {
        &*(addr as *const Self)
    }

    pub fn magic_valid(&self) -> bool {
        self.magic == 0xedfe0dd0
    }

    pub fn total_size(&self) -> u32 { self.total_size.swap_bytes() }
    pub fn off_dt_struct(&self) -> u32 { self.off_dt_struct.swap_bytes() }
    pub fn off_dt_strings(&self) -> u32 { self.off_dt_strings.swap_bytes() }
    pub fn off_mem_rsvmap(&self) -> u32 { self.off_mem_rsvmap.swap_bytes() }
    pub fn version(&self) -> u32 { self.version.swap_bytes() }
    pub fn last_comp_version(&self) -> u32 { self.last_comp_version.swap_bytes() }
    pub fn boot_cpuid_phys(&self) -> u32 { self.boot_cpuid_phys.swap_bytes() }
    pub fn size_dt_strings(&self) -> u32 { self.size_dt_strings.swap_bytes() }
    pub fn size_dt_struct(&self) -> u32 { self.size_dt_struct.swap_bytes() }

    pub fn address(&self) -> *const u8 { self as *const _ as *const u8 }

    pub unsafe fn memory_reservations(&self) -> &'static [MemoryRegion] {
        let addr = self.address().offset(self.off_mem_rsvmap() as isize) as *const MemoryRegion;
        let mut entries = 0;
        loop {
            let entry = &*addr.offset(entries as isize);
            if entry.size() == 0 && entry.offset() == 0 {
                break;
            }
            entries += 1;
        }

        core::slice::from_raw_parts(addr, entries)
    }

    pub unsafe fn strings(&self) -> &'static [u8] {
        let addr = self.address().offset(self.off_dt_strings() as isize);
        core::slice::from_raw_parts(addr, self.size_dt_strings() as usize)
    }

    pub unsafe fn get_string(&self, offset: u32) -> &'static str {
        self.str_from_ptr(self.address().offset((self.off_dt_strings() + offset) as isize))
    }
    pub unsafe fn str_from_ptr(&self, start: *const u8) -> &'static str {
        let mut ptr = start;
        while *ptr != 0 {
            ptr = ptr.add(1);
        }

        core::str::from_utf8(core::slice::from_raw_parts(start, ptr.offset_from(start) as usize)).unwrap()
    }

    pub unsafe fn print(&self) {
        self.walk(|path, unit_addresses, v| match v {
            FdtVisit::Property { name, prop } => {
                if path.len() == 1 {
                    print!("[root]");
                }
                for i in 1..path.len() {
                    print!("/{}", path[i]);
                    if unit_addresses[i] != "" {
                        print!("@{}", unit_addresses[i]);
                    }
                }
                print!(":{}", name);

                if prop.len() == 4 || prop.len() == 8 {
                    println!("={:#x}", prop.read_int());
                } else if prop.len() == 16 {
                    let range = prop.read_range();
                    println!("={:x}:{:x}", range.0, range.1);
                } else if prop.len() != 0 {
                    if let Some(value) = prop.value_str() {
                        println!("=\"{}\"", value);
                    } else {
                        println!(" (value_len={})", prop.len());
                    }
                } else {
                    println!("");
                }
            }
            FdtVisit::Node { .. } => {}
        });
    }

    pub unsafe fn parse(&self) -> MachineMeta {
        let mut initrd_start: Option<u64> = None;
        let mut initrd_end: Option<u64> = None;
        let mut plic: Option<u64> = None;
        let mut clint: Option<u64> = None;

        let mut meta = MachineMeta::default();

        let mut virtio_address_map = AddressMap::default();
        let mut virtio = [(None, None); AddressMap::MAX_LEN];

        // (hartid, phandle)
        let mut cpus = [(None, None); AddressMap::MAX_LEN];
        let mut cpu_address_map = AddressMap::default();

        // hart phandle for each plic S-mode context
        let mut plic_context_phandles = [None; 64];

        self.walk(|path, unit_addresses, v| {
            match v {
                FdtVisit::Property { name, prop } => match (path, name) {
                    (["", "chosen"], "linux,initrd-end") => initrd_end = Some(prop.read_int()),
                    (["", "chosen"], "linux,initrd-start") => initrd_start = Some(prop.read_int()),
                    (["", "memory"], "reg") => {
                        assert_eq!(prop.len(), 16);
                        let region = prop.address().offset(8) as *const _ as *mut MemoryRegion;
                        meta.physical_memory_offset = (*region).offset();
                        meta.physical_memory_size = (*region).size();
                    }
                    (["", "uart"], "reg") |
                    (["", "soc", "uart"], "reg") |
                    (["", "soc", "serial"], "reg") => if meta.uart_address == 0 {
                        meta.uart_address = prop.read_range().0
                    }
                    (["", "uart"], "compatible") |
                    (["", "soc", "uart"], "compatible") |
                    (["", "soc", "serial"], "compatible") => if meta.uart_type.is_none() {
                        match prop.value_str().map(|s| s.trim_end_matches('\0')) {
                            Some("ns16550a") => meta.uart_type = Some(UartType::Ns16550a),
                            Some("sifive,uart0") => meta.uart_type = Some(UartType::SiFive),
                            _ => {},
                        }
                    }
                    (["", "soc", "clint"], "reg") => clint = Some(prop.read_range().0),
                    (["", "soc", "interrupt-controller"], "reg") => plic = Some(prop.read_range().0),
                    (["", "soc", "interrupt-controller"], "interrupts-extended") => {
                        let s: &[u32] = prop.big_endian_slice();
                        for i in 0..(s.len()/2) {
                            let irq = s[i*2 + 1].swap_bytes();
                            if irq == 9 {
                                plic_context_phandles[i] = Some(s[i*2].swap_bytes());
                            }
                        }
                    }
                    (["", "virtio_mmio"], "reg") => {
                        let index = virtio_address_map.index_of(unit_addresses[1]);
                        virtio[index].0 = Some(prop.read_range());
                    }
                    (["", "virtio_mmio"], "interrupts") => {
                        let index = virtio_address_map.index_of(unit_addresses[1]);
                        virtio[index].1 = Some(prop.read_int());
                    }
                    (["", "cpus", "cpu"], "reg") => {
                        let index = virtio_address_map.index_of(unit_addresses[2]);
                        cpus[index].0 = Some(prop.read_int());
                    }
                    (["", "cpus", "cpu", "interrupt-controller"], "phandle") => {
                        let index = virtio_address_map.index_of(unit_addresses[2]);
                        cpus[index].1 = Some(prop.read_int());
                    }
                    _ => {},
                }
                FdtVisit::Node { .. } => {}
            }
        });

        if initrd_start.is_some() && initrd_end.is_some() {
            meta.initrd_start = initrd_start.unwrap();
            meta.initrd_end = initrd_end.unwrap();
        }

        meta.plic_address = plic.unwrap();
        meta.clint_address = clint.unwrap();

        for &c in cpus.iter() {
            if let (Some(hartid), Some(phandle)) = c {
                if let Some(plic_context) = plic_context_phandles.iter().position(|&p| p == Some(phandle as u32)) {
                    meta.harts.push(Hart {
                        hartid,
                        plic_context: plic_context as u64,
                    })
                }
            }
        }
        meta.harts.sort_unstable_by_key(|h|h.hartid);

        for &v in virtio.iter().rev() {
            if let (Some((base_address, size)), Some(irq)) = v {
                meta.virtio.push(Device {
                    base_address,
                    size,
                    irq
                })
            }
        }
        meta.virtio.sort_unstable_by_key(|v| v.base_address);

        meta
    }

    pub unsafe fn mask(&self, guest_memory_size: u64) {
        self.walk(|path, unit_addresses, v| match v {
            FdtVisit::Property { name, prop } => match (path, name) {
                (["", "chosen"], "linux,initrd-end") => prop.mask(),
                (["", "chosen"], "linux,initrd-start") => prop.mask(),
                (["", "memory"], "reg") => {
                    let region = prop.address().offset(8) as *const _ as *mut MemoryRegion;
                    (*region).set_size(guest_memory_size);
                }
                _ => {},
            }
            FdtVisit::Node { mask } => *mask = match path {
                ["", "cpus", "cpu"] => (unit_addresses[2] != "" && unit_addresses[2] != "0"),
                ["", "soc", "pci"] => true,
                ["", "test"] => true,
                ["", "virtio_mmio"] if unit_addresses[1] == "10005000" => true,
                ["", "virtio_mmio"] if unit_addresses[1] == "10006000" => true,
                ["", "virtio_mmio"] if unit_addresses[1] == "10007000" => true,
                ["", "virtio_mmio"] if unit_addresses[1] == "10008000" => true,
                _ => false,
            },
        });
    }

    // Mask out entries from FDT and return some information about the machine.
    unsafe fn walk<F>(&self, mut visit: F) where
        F: FnMut(&[&str], &[&str], FdtVisit),
    {
        let mut mask_node = 0;

        let mut path = ArrayVec::<[_; 16]>::new();
        let mut unit_addresses = ArrayVec::<[_; 16]>::new();

        let mut ptr = self.address().offset(self.off_dt_struct() as isize) as *const u32;
        let end = ptr.offset((self.size_dt_struct() as isize + 3) / 4);
        while ptr < end && *ptr != FDT_END {
            let old_ptr = ptr;
            match *ptr {
                FDT_BEGIN_NODE => {
                    ptr = ptr.add(1);
                    let full_name = self.str_from_ptr(ptr as *const u8);
                    ptr = ptr.add(1 + full_name.len() / 4);

                    let mut name_parts = full_name.split('@');
                    path.push(name_parts.next().unwrap_or(""));
                    unit_addresses.push(name_parts.next().unwrap_or(""));

                    if mask_node > 0 {
                        mask_node += 1;
                    } else {
                        let mut mask = false;
                        visit(&path, &unit_addresses, FdtVisit::Node { mask: &mut mask });
                        if mask {
                            mask_node = 1;
                        }
                    }
                }
                FDT_END_NODE => {
                    if mask_node > 0 {
                        *(ptr as *mut u32) = FDT_NOP;
                        mask_node = mask_node - 1;
                    }
                    path.pop();
                    unit_addresses.pop();
                    ptr = ptr.offset(1);
                }
                FDT_PROP => {
                    let prop = &*(ptr.offset(1) as *const Property);
                    let prop_name = self.get_string(prop.name_offset());
                    ptr = ptr.offset(3 + (prop.len() as isize + 3) / 4);
                    visit(&path, &unit_addresses, FdtVisit::Property{ name: prop_name, prop });
                }
                FDT_NOP | _ => ptr = ptr.offset(1),
            }

            if mask_node > 0 {
                for i in 0..ptr.offset_from(old_ptr) {
                    *(old_ptr.offset(i) as *mut u32) = FDT_NOP;
                }
            }
        }
    }
}

#[repr(C)]
#[derive(Clone)]
pub struct MemoryRegion([u32; 4]);
impl MemoryRegion {
    pub fn offset(&self) -> u64 {
        unsafe { ptr::read_unaligned(self as *const _ as *const u64).swap_bytes() }
    }
    pub fn size(&self) -> u64 {
        unsafe { ptr::read_unaligned((self as *const _ as *const u64).offset(1)).swap_bytes() }
    }
    pub fn set_size(&mut self, size: u64) {
        unsafe { ptr::write_unaligned((self as *mut _ as *mut u64).offset(1), size.swap_bytes()) }
    }
}

#[repr(C)]
pub struct Property {
    len: u32,
    name_offset: u32,
}
impl Property {
    pub fn len(&self) -> u32 { self.len.swap_bytes() }
    pub fn name_offset(&self) -> u32 { self.name_offset.swap_bytes() }

    pub fn address(&self) -> *const u8 { self as *const _ as *const u8 }

    pub unsafe fn read_int(&self) -> u64 {
        match self.len() {
            4 => (*(self.address().add(8) as *const u32)).swap_bytes() as u64,
            8 => (ptr::read_unaligned(self.address().add(8) as *const u64)).swap_bytes(),
            _ => unreachable!(),
        }
    }
    pub unsafe fn read_range(&self) -> (u64, u64) {
        assert_eq!(self.len(), 16);
        (
            (ptr::read_unaligned(self.address().add(8) as *const u64)).swap_bytes(),
            (ptr::read_unaligned(self.address().add(16) as *const u64)).swap_bytes()
        )
    }
    pub unsafe fn mask(&self) {
        let length = (self.len() as usize + 3) / 4 + 3;
        let start = self.address().offset(-4) as *const u32 as *mut u32;

        for i in 0..length {
            *(start.add(i)) = FDT_NOP;
        }
    }
    pub unsafe fn value_str(&self) -> Option<&str> {
        if self.len() == 0 { return Some(""); }

        for i in 0..(self.len() - 1) {
            let c = *self.address().add(8 + i as usize);
            if c < 32 || c > 126 {
                return None;
            }
        }
        core::str::from_utf8(core::slice::from_raw_parts(self.address().add(8), self.len() as usize)).ok()
    }
    pub unsafe fn big_endian_slice<T: Sized>(&self) -> &[T] {
        core::slice::from_raw_parts(self.address().add(8) as *const T, self.len() as usize / mem::size_of::<T>())
    }
}

enum FdtVisit<'a> {
    Node { mask: &'a mut bool },
    Property {
        name: &'a str,
        prop: &'a Property,
    }
}
