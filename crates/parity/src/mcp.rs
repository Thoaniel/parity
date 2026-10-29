//! `parity mcp`: Parity's tools for any agent over the Model Context Protocol (JSON-RPC 2.0, one message per line
//! on stdin/stdout). Analysis tools read files; `surface_map` and `replay` run programs in the project's sandbox.

use crate::project::{Layout, Program};
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

const PROTOCOL: &str = "2025-06-18";

pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&msg),
            Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(reply) = reply {
            serde_json::to_writer(&mut stdout, &reply)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// The reply to one message; notifications get none.
fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match msg.get("method").and_then(Value::as_str).unwrap_or("") {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "parity", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Parity: inspect executables (overview, sections, symbols, strings, disassembly), map a \
                program's surface (flags, subcommands, env vars, paths), and check a rewrite against recorded \
                behaviour (replay). Paths are local file paths."
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Ok(match call(name, &args) {
                Ok(v) => json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }],
                    "isError": false
                }),
                Err(e) => {
                    json!({ "content": [{ "type": "text", "text": format!("{e:#}") }], "isError": true })
                }
            })
        }
        other => Err((-32601, format!("unknown method {other}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => error(id, code, &message),
    })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required }
    })
}

fn tools() -> Vec<Value> {
    let path = json!({ "type": "string", "description": "Path of the file to analyse" });
    vec![
        tool(
            "binary_overview",
            "Format, architecture, entry point, libraries, imports, exports and toolchain hints of an executable \
             or object file (ELF, PE, Mach-O).",
            json!({ "path": path }),
            &["path"],
        ),
        tool(
            "list_sections",
            "Sections with addresses, sizes, file offsets and kinds.",
            json!({ "path": path }),
            &["path"],
        ),
        tool(
            "list_symbols",
            "Static and dynamic symbols with demangled names (Rust, C++), addresses and sizes.",
            json!({
                "path": path,
                "filter": { "type": "string", "description": "Only names containing this" },
                "limit": { "type": "integer", "description": "At most this many (default 500, 0 = all)" }
            }),
            &["path"],
        ),
        tool(
            "list_strings",
            "Printable strings (UTF-8 and UTF-16LE) of any file, with offsets.",
            json!({
                "path": path,
                "min_len": { "type": "integer", "description": "Minimum length (default 6)" },
                "pattern": { "type": "string", "description": "Only strings matching this regex" },
                "limit": { "type": "integer", "description": "At most this many (default 500, 0 = all)" }
            }),
            &["path"],
        ),
        tool(
            "disassemble",
            "Disassemble from a symbol (name or demangled name) or an address, for any architecture the file declares.",
            json!({
                "path": path,
                "symbol": { "type": "string" },
                "address": { "type": "string", "description": "Hex (0x...) or decimal" },
                "count": { "type": "integer", "description": "Instructions (default 64)" }
            }),
            &["path"],
        ),
        tool(
            "surface_map",
            "What a program exposes and depends on: flags, subcommands, env vars, config paths, URLs, from its file, \
             its source (Python, Node) and, unless run_help is false, its own --help run in a sandbox.",
            json!({
                "program": { "type": "string", "description": "Name on PATH or path" },
                "run_help": { "type": "boolean" }
            }),
            &["program"],
        ),
        tool(
            "replay",
            "Run a candidate program on a Parity project's recorded cases and report every divergence from the \
             original. The judge for a rewrite: it says exactly what is still wrong.",
            json!({
                "project": { "type": "string", "description": "Project folder (with parity.toml)" },
                "candidate": { "type": "string", "description": "Program to check (default: the project's)" },
                "cases": { "type": "array", "items": { "type": "string" }, "description": "Only these cases" }
            }),
            &["project"],
        ),
    ]
}

fn call(name: &str, args: &Value) -> Result<Value> {
    let s = |k: &str| args.get(k).and_then(Value::as_str);
    let n = |k: &str, default: u64| args.get(k).and_then(Value::as_u64).unwrap_or(default) as usize;
    let file = || -> Result<Vec<u8>> {
        let path = s("path").context("path is required")?;
        std::fs::read(path).with_context(|| path.to_string())
    };
    let limited = |mut v: Vec<Value>, limit: usize| {
        let total = v.len();
        if limit > 0 {
            v.truncate(limit);
        }
        json!({ "total": total, "items": v })
    };
    Ok(match name {
        "binary_overview" => serde_json::to_value(parity_rea::overview(&file()?)?)?,
        "list_sections" => serde_json::to_value(parity_rea::sections(&file()?)?)?,
        "list_symbols" => {
            let syms = parity_rea::symbols(&file()?, s("filter"))?;
            limited(
                syms.iter()
                    .map(serde_json::to_value)
                    .collect::<Result<_, _>>()?,
                n("limit", 500),
            )
        }
        "list_strings" => {
            let re = s("pattern").map(regex::Regex::new).transpose()?;
            let found = parity_rea::strings(&file()?, n("min_len", 6), re.as_ref(), 0);
            limited(
                found
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<Result<_, _>>()?,
                n("limit", 500),
            )
        }
        "disassemble" => {
            let data = file()?;
            let start = match (s("symbol"), s("address")) {
                (Some(sym), _) => parity_rea::disasm::Start::Symbol(sym),
                (None, Some(a)) => parity_rea::disasm::Start::Address(parse_address(a)?),
                (None, None) => return Err(anyhow!("symbol or address is required")),
            };
            serde_json::to_value(parity_rea::disassemble(&data, start, n("count", 64))?)?
        }
        "surface_map" => {
            let program = Program {
                program: s("program").context("program is required")?.to_string(),
                args: Vec::new(),
            };
            let opts = crate::discover::Options {
                run_help: args
                    .get("run_help")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                max_subcommands: 50,
            };
            serde_json::to_value(crate::discover::discover(
                &program,
                &Default::default(),
                &opts,
            )?)?
        }
        "replay" => {
            let layout = Layout::new(s("project").context("project is required")?);
            let cases: Vec<String> = args
                .get("cases")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let report =
                crate::replay_report(&layout, s("candidate").map(str::to_string), &cases, false)?;
            json!({ "matching": report.matching(), "total": report.cases.len(), "report": report })
        }
        other => return Err(anyhow!("unknown tool {other}")),
    })
}

fn parse_address(text: &str) -> Result<u64> {
    let t = text.trim();
    Ok(
        match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            Some(hex) => u64::from_str_radix(hex, 16)?,
            None => t.parse()?,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_initialize_list_and_unknown_methods() {
        let init = handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}))
            .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        let list = handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        assert!(list["result"]["tools"].as_array().unwrap().len() >= 7);
        assert!(handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        let bad = handle(&json!({"jsonrpc":"2.0","id":3,"method":"nope"})).unwrap();
        assert_eq!(bad["error"]["code"], -32601);
    }

    #[test]
    fn tool_errors_are_results_not_protocol_errors() {
        let r = handle(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"binary_overview","arguments":{}}}))
            .unwrap();
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn analyses_its_own_binary() {
        let me = std::env::current_exe().unwrap().display().to_string();
        let v = call("binary_overview", &json!({ "path": me })).unwrap();
        assert!(v["sections"].as_u64().unwrap() > 0);
        assert_eq!(parse_address("0x10").unwrap(), 16);
        assert_eq!(parse_address("16").unwrap(), 16);
    }
}
