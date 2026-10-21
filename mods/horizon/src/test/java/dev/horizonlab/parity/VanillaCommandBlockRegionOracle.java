package dev.horizonlab.parity;

import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.core.BlockPos;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.entity.CommandBlockEntity;
import net.minecraft.world.level.chunk.storage.RegionFile;
import net.minecraft.world.level.chunk.storage.RegionStorageInfo;

/** Native vanilla 26.3 RegionFile + NbtIo + BlockEntity loader validation
 * of an unmodified .mca region file produced by Horizon's real SaveDir writer. */
public final class VanillaCommandBlockRegionOracle {
    public static void main(String[] args) throws Exception {
        if(args.length != 1)throw new IllegalArgumentException("usage: REGION_ROOT");
        Path regionFolder=Path.of(args[0]).resolve("region");
        Path filename=regionFolder.resolve("r.-1.0.mca");
        if(!Files.isRegularFile(filename))throw new IllegalStateException("missing Horizon mca "+filename);
        ParityData.init();
        CompoundTag chunk;
        try(var region=new RegionFile(new RegionStorageInfo("horizon-parity",Level.OVERWORLD,"chunk"),
                                     filename,regionFolder,false)) {
            try(var in=region.getChunkDataInputStream(new ChunkPos(-1,1))) {
                if(in==null)throw new IllegalStateException("vanilla RegionFile cannot read Horizon chunk");
                chunk=NbtIo.read(in);
            }
        }
        if(chunk==null)throw new IllegalStateException("vanilla NbtIo returned no data");
        if(!Integer.valueOf(-1).equals(chunk.getIntOr("xPos",999))) throw new AssertionError("wrong xPos");
        if(!Integer.valueOf(1).equals(chunk.getIntOr("zPos",999))) throw new AssertionError("wrong zPos");
        var entities=chunk.get("block_entities");
        if(!(entities instanceof ListTag list) || list.size()!=1)throw new AssertionError("expected single command block NBT: "+entities);
        if(!(list.get(0) instanceof CompoundTag raw))throw new AssertionError("not a compound command block");
        if(!"minecraft:command_block".equals(raw.getStringOr("id","")))throw new AssertionError("wrong block entity ID");
        var pos=new BlockPos(-1,64,16);
        BlockEntity loaded=BlockEntity.loadStatic(pos, Blocks.COMMAND_BLOCK.defaultBlockState(), raw, ParityData.lookup);
        if(!(loaded instanceof CommandBlockEntity cb))throw new AssertionError("vanilla BlockEntity.loadStatic failed");
        if(!"Searge".equals(cb.getCommandBlock().getCommand()))throw new AssertionError("command changed");
        if(cb.getCommandBlock().getSuccessCount()!=1)throw new AssertionError("success count changed");
        if(!"updated".equals(cb.getCommandBlock().getLastOutput().getString()))throw new AssertionError("last output changed");
        if(!cb.isPowered())throw new AssertionError("powered flag lost");
        var savedAgain=cb.saveCustomOnly(ParityData.lookup);
        if(savedAgain.getLongOr("LastExecution",-1L)!=42)throw new AssertionError("last execution timestamp changed");
        var value=savedAgain.get("LastOutput");
        var expected="{color:\"red\",text:\"updated\"}";
        if(!expected.equals(ParityData.canon(value)))throw new AssertionError("rich output changed: "+ParityData.canon(value));
        var ticks=chunk.get("block_ticks");
        if(!(ticks instanceof ListTag pending) || pending.size()!=1)throw new AssertionError("scheduled tick lost: "+ticks);
        if(!(pending.get(0) instanceof CompoundTag tick))throw new AssertionError("not a scheduled tick compound");
        if(!"minecraft:command_block".equals(tick.getStringOr("i",""))
            ||tick.getIntOr("x",0)!=-1 ||tick.getIntOr("y",0)!=64
            ||tick.getIntOr("z",0)!=16 ||tick.getIntOr("t",-1)!=7
            ||tick.getIntOr("p",0)!=-1)throw new AssertionError("scheduled tick fields changed: "+ParityData.canon(tick));
        System.out.println("VANILLA_REGION_FILE_PASS: RegionFile.getChunkDataInputStream, NbtIo.read, BlockEntity.loadStatic, styled output and saved tick all verified");
    }
}
