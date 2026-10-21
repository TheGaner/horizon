//! Exact limited field-level command-block NBT fixture against vanilla Java 26.3.
//! Calls production CommandBlock::{save,load}, not a handcrafted Rust serializer.
use horizon_client::command_block::CommandBlock;
use horizon_client::text::Text;
use horizon_world::nbt::Tag;
use std::{env, fs};

const KEYS: [&str; 10] = [
    "Command",
    "SuccessCount",
    "CustomName",
    "TrackOutput",
    "LastOutput",
    "UpdateLastExecution",
    "LastExecution",
    "powered",
    "auto",
    "conditionMet",
];

fn canonical(tag: Option<&Tag>) -> String {
    match tag {
        None => "ABSENT".to_owned(),
        Some(Tag::Byte(v)) => format!("{v}b"),
        Some(Tag::Int(v)) => v.to_string(),
        Some(Tag::Long(v)) => format!("{v}L"),
        Some(Tag::String(v)) => {
            let text = v.to_string().expect("fixture UTF-16 string");
            format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
        }
        Some(Tag::Compound(fields)) => {
            let entries = fields
                .iter()
                .map(|(key, value)| format!("{}:{}", key.to_string_lossy(), canonical(Some(value))))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{entries}}}")
        }
        other => panic!("unsupported NBT type in fixture: {other:?}"),
    }
}
fn snapshot(tag: &Tag) -> String {
    KEYS.iter()
        .map(|key| format!("{key}={}", canonical(tag.get(key))))
        .collect::<Vec<_>>()
        .join("|")
}
fn parse_bool(s: &str) -> bool {
    match s {
        "true" => true,
        "false" => false,
        _ => panic!("invalid boolean {s}"),
    }
}
fn main() {
    let args: Vec<String> = env::args().collect();
    assert_eq!(
        args.len(),
        3,
        "usage: command_block_nbt_fixture INPUT OUTPUT"
    );
    let mut results = String::new();
    for line in fs::read_to_string(&args[1]).expect("read fixtures").lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<_> = line.split('|').collect();
        assert_eq!(parts.len(), 11, "invalid fixture: {line}");
        let mut block = CommandBlock::default();
        block.command = if parts[0] == "~" {
            String::new()
        } else {
            parts[0].into()
        };
        block.success_count = parts[1].parse().expect("success count");
        if parts[2] != "~" {
            block.custom_name = Some(if let Some(value) = parts[2].strip_prefix("@red:") {
                Tag::compound([
                    ("color", Tag::String("red".into())),
                    ("text", Tag::String(value.into())),
                ])
            } else {
                Tag::String(parts[2].into())
            });
        }
        block.track_output = parse_bool(parts[3]);
        if parts[4] != "~" {
            block.last_output = Some(if let Some(value) = parts[4].strip_prefix("@red:") {
                Text::colored(value, horizon_client::text::RED)
            } else {
                Text::literal(parts[4])
            });
        }
        block.update_last_execution = parse_bool(parts[5]);
        block.last_execution = parts[6].parse().expect("last execution");
        block.powered = parse_bool(parts[7]);
        block.auto = parse_bool(parts[8]);
        block.condition_met = parse_bool(parts[9]);
        let full = parse_bool(parts[10]);
        let mut saved = Tag::compound(block.save());
        if full {
            if let Tag::Compound(fields) = &mut saved {
                fields.insert("id".into(), Tag::String("minecraft:command_block".into()));
                fields.insert("x".into(), Tag::Int(7));
                fields.insert("y".into(), Tag::Int(63));
                fields.insert("z".into(), Tag::Int(-11));
            }
        }
        let restored = CommandBlock::load(&saved);
        let mut resaved = Tag::compound(restored.save());
        if full {
            if let Tag::Compound(fields) = &mut resaved {
                fields.insert("id".into(), Tag::String("minecraft:command_block".into()));
                fields.insert("x".into(), Tag::Int(7));
                fields.insert("y".into(), Tag::Int(63));
                fields.insert("z".into(), Tag::Int(-11));
            }
        }
        results.push_str(&snapshot(&saved));
        results.push('#');
        results.push_str(&snapshot(&resaved));
        results.push('\n');
    }
    fs::write(&args[2], results).expect("write output");
}
