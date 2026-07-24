use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

use pdb::{FallibleIterator, PDB, SymbolData};
use uuid::Uuid;

use super::pe::CodeViewInfo;

#[derive(Debug, Clone)]
pub struct PublicSymbol {
    pub name: String,
    pub rva: u32,
}

pub struct PdbPublics {
    pub guid: Uuid,
    pub age: u32,
    pub symbols: Vec<PublicSymbol>,
    pub rva_names: HashMap<u32, Vec<String>>,
}

impl PdbPublics {
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = File::open(path)
            .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
        let mut pdb = PDB::open(file)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
        let info = pdb
            .pdb_information()
            .map_err(|error| format!("failed to read PDB information: {error}"))?;
        let dbi = pdb
            .debug_information()
            .map_err(|error| format!("failed to read PDB DBI stream: {error}"))?;
        let age = dbi.age().unwrap_or(info.age);
        let address_map = pdb
            .address_map()
            .map_err(|error| format!("failed to read PDB address map: {error}"))?;
        let symbol_table = pdb
            .global_symbols()
            .map_err(|error| format!("failed to read PDB global symbols: {error}"))?;

        let mut symbols = Vec::new();
        let mut rva_names: HashMap<u32, Vec<String>> = HashMap::new();
        let mut iter = symbol_table.iter();
        while let Some(symbol) = iter
            .next()
            .map_err(|error| format!("failed to iterate PDB symbols: {error}"))?
        {
            let Ok(SymbolData::Public(public)) = symbol.parse() else {
                continue;
            };
            let Some(rva) = public.offset.to_rva(&address_map) else {
                continue;
            };
            let name = public.name.to_string().into_owned();
            rva_names.entry(rva.0).or_default().push(name.clone());
            symbols.push(PublicSymbol { name, rva: rva.0 });
        }

        Ok(Self {
            guid: info.guid,
            age,
            symbols,
            rva_names,
        })
    }

    pub fn verify_against_pe(&self, pe_cv: &CodeViewInfo) -> Result<(), String> {
        if self.guid != pe_cv.guid {
            return Err(format!(
                "PE/PDB CodeView GUID mismatch: pe={} pdb={}",
                pe_cv.guid, self.guid
            ));
        }
        if self.age != pe_cv.age {
            return Err(format!(
                "PE/PDB CodeView age mismatch: pe={} pdb={}",
                pe_cv.age, self.age
            ));
        }
        Ok(())
    }

    pub fn find_by_prefix(&self, prefix: &str) -> Vec<&PublicSymbol> {
        self.symbols
            .iter()
            .filter(|symbol| symbol.name.starts_with(prefix))
            .collect()
    }
}

pub fn is_icf_false_stub(code: &[u8]) -> bool {
    matches!(
        code,
        [0x32, 0xC0, 0xC3, ..] | [0x33, 0xC0, 0xC3, ..] | [0xB0, 0x00, 0xC3, ..]
    )
}
