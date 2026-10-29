//! Disassembly with Capstone, for the architecture the file declares: x86, x86-64, ARM, AArch64, RISC-V, MIPS,
//! PowerPC and SPARC. Start at a symbol (by name or demangled name) or at an address.

use anyhow::{Context, Result, bail};
use capstone::arch::{self, BuildsCapstone, BuildsCapstoneEndian, BuildsCapstoneExtraMode};
use capstone::{Capstone, Endian};
use object::{Architecture, Object, ObjectSection, ObjectSymbol};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Instruction {
    pub address: u64,
    pub bytes: String,
    pub mnemonic: String,
    pub operands: String,
}

pub enum Start<'a> {
    Symbol(&'a str),
    Address(u64),
}

/// Up to `count` instructions from `start`. A symbol with a known size stops at its end.
pub fn disassemble(data: &[u8], start: Start, count: usize) -> Result<Vec<Instruction>> {
    let file = object::File::parse(data)?;
    let (address, end) = match start {
        Start::Address(a) => (a, None),
        Start::Symbol(name) => {
            let sym = file
                .symbols()
                .chain(file.dynamic_symbols())
                .find(|s| {
                    s.name()
                        .is_ok_and(|n| n == name || crate::demangle(n) == name)
                        && s.address() != 0
                })
                .with_context(|| format!("no symbol {name:?} with an address"))?;
            let end = (sym.size() > 0).then(|| sym.address() + sym.size());
            (sym.address(), end)
        }
    };
    let section = file
        .sections()
        .find(|s| s.address() <= address && address < s.address() + s.size())
        .with_context(|| format!("address {address:#x} is in no section"))?;
    let bytes = section.data()?;
    let from = (address - section.address()) as usize;
    let to = end
        .map(|e| (e - section.address()) as usize)
        .unwrap_or(bytes.len())
        .min(bytes.len());
    if from >= to {
        bail!(
            "section {} has no file data at {address:#x}",
            section.name().unwrap_or("?")
        );
    }
    let cs = capstone_for(file.architecture(), file.is_little_endian())?;
    let insns = cs.disasm_count(&bytes[from..to], address, count)?;
    Ok(insns
        .iter()
        .map(|i| Instruction {
            address: i.address(),
            bytes: i.bytes().iter().map(|b| format!("{b:02x}")).collect(),
            mnemonic: i.mnemonic().unwrap_or("").to_string(),
            operands: i.op_str().unwrap_or("").to_string(),
        })
        .collect())
}

fn capstone_for(arch: Architecture, little: bool) -> Result<Capstone> {
    let endian = if little { Endian::Little } else { Endian::Big };
    let cs = match arch {
        Architecture::X86_64 | Architecture::X86_64_X32 => Capstone::new()
            .x86()
            .mode(arch::x86::ArchMode::Mode64)
            .build(),
        Architecture::I386 => Capstone::new()
            .x86()
            .mode(arch::x86::ArchMode::Mode32)
            .build(),
        Architecture::Aarch64 | Architecture::Aarch64_Ilp32 => Capstone::new()
            .arm64()
            .mode(arch::arm64::ArchMode::Arm)
            .endian(endian)
            .build(),
        Architecture::Arm => Capstone::new()
            .arm()
            .mode(arch::arm::ArchMode::Arm)
            .endian(endian)
            .build(),
        Architecture::Riscv64 => Capstone::new()
            .riscv()
            .mode(arch::riscv::ArchMode::RiscV64)
            .extra_mode([arch::riscv::ArchExtraMode::RiscVC].into_iter())
            .build(),
        Architecture::Riscv32 => Capstone::new()
            .riscv()
            .mode(arch::riscv::ArchMode::RiscV32)
            .extra_mode([arch::riscv::ArchExtraMode::RiscVC].into_iter())
            .build(),
        Architecture::Mips => Capstone::new()
            .mips()
            .mode(arch::mips::ArchMode::Mips32)
            .endian(endian)
            .build(),
        Architecture::Mips64 => Capstone::new()
            .mips()
            .mode(arch::mips::ArchMode::Mips64)
            .endian(endian)
            .build(),
        Architecture::PowerPc => Capstone::new()
            .ppc()
            .mode(arch::ppc::ArchMode::Mode32)
            .endian(endian)
            .build(),
        Architecture::PowerPc64 => Capstone::new()
            .ppc()
            .mode(arch::ppc::ArchMode::Mode64)
            .endian(endian)
            .build(),
        Architecture::Sparc64 => Capstone::new()
            .sparc()
            .mode(arch::sparc::ArchMode::V9)
            .build(),
        other => bail!("no disassembler for {other:?} yet"),
    };
    Ok(cs?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disassembles_a_function_of_this_test_binary() {
        let data = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let name = crate::symbols(&data, Some("disassembles_a_function_of_this_test_binary"))
            .unwrap()
            .into_iter()
            .find(|s| s.defined && s.address != 0)
            .unwrap()
            .name;
        let insns = disassemble(&data, Start::Symbol(&name), 8).unwrap();
        assert!(!insns.is_empty());
        assert!(insns.iter().all(|i| !i.mnemonic.is_empty()));
    }
}
