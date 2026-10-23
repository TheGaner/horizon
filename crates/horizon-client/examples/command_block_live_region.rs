//! Inspect a real vanilla 26.3 command block in a disk .mca through Horizon's NBT reader.
//! Validates the production CommandBlock::load/save component codec, including
//! the nested translated LastOutput that actual commands produce.
use horizon_client::command_block::CommandBlock;
use horizon_core::position::ChunkPos;
use horizon_world::{
    nbt::{Limits, Tag},
    region::RegionFile,
};
use std::{env, path::Path};

fn main() {
    let args: Vec<_> = env::args_os().collect();
    assert_eq!(
        args.len(),
        2,
        "usage: command_block_live_region PATH_TO_REGION_FILE"
    );
    let path = Path::new(&args[1]);
    let mut region = RegionFile::open(path).expect("open unmodified vanilla region");
    let chunk = region
        .read(ChunkPos::new(6, 6))
        .expect("read chunk")
        .expect("chunk 6,6 exists");
    let named = chunk.nbt(Limits::default()).expect("decode region NBT");
    let Some(Tag::List(entities)) = named.tag.get("block_entities") else {
        panic!("no block_entities");
    };
    let saved = entities.iter().find(|e|
        matches!(e.get("id"), Some(Tag::String(s)) if s.to_string_lossy() == "minecraft:command_block")
        && e.get("x") == Some(&Tag::Int(96))
        && e.get("y") == Some(&Tag::Int(64))
        && e.get("z") == Some(&Tag::Int(96))).expect("command block in chunk");
    let block = CommandBlock::load(saved);
    assert_eq!(block.command, "scoreboard players add #probe cbhit 1");
    assert_eq!(block.success_count, 1);
    assert!(block.auto);
    assert!(block.last_execution >= 0);
    let output = Tag::compound(block.save());
    let before = saved.get("LastOutput");
    let after = output.get("LastOutput");
    assert!(
        before.is_some(),
        "vanilla has no LastOutput: chunk wasn't saved dirty"
    );
    println!(
        "last_output_parity={} last_execution={} success_count={} rich={}",
        before == after,
        block.last_execution,
        block.success_count,
        before.is_some()
    );
    if before != after {
        eprintln!("ORIGINAL={before:?}\nHORIZON_RESAVED={after:?}");
        std::process::exit(1);
    }
    println!(
        "PASS: production Horizon Rust read real vanilla Java 26.3 .mca command block and losslessly reencoded nested LastOutput"
    );
}
