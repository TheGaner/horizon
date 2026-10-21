package dev.horizonlab.parity;

import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.ChatFormatting;
import net.minecraft.core.BlockPos;
import net.minecraft.network.chat.Component;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.CommandBlockEntity;

/** Bytecode-backed command-block NBT serialization and complete save/reload. */
public final class VanillaCommandBlockNbtOracle {
    private static final BlockPos POS = new BlockPos(7,63,-11);
    private static Field lastExecution;
    private static Field updateLastExecution;
    private static Field conditionMet;

    private static CommandBlockEntity fresh() {
        return new CommandBlockEntity(POS,Blocks.COMMAND_BLOCK.defaultBlockState());
    }

    private static String inspect(CompoundTag tag) {
        var fields = new StringBuilder();
        for (String key : new String[]{"Command","SuccessCount","CustomName","TrackOutput","LastOutput",
            "UpdateLastExecution","LastExecution","powered","auto","conditionMet"}) {
            if(fields.length()>0) fields.append('|');
            var value=tag.get(key);
            fields.append(key).append('=');
            fields.append(value==null?"ABSENT":ParityData.canon(value));
        }
        return fields.toString();
    }

    public static void main(String[] args) throws Exception {
        ParityData.init();
        lastExecution=net.minecraft.world.level.BaseCommandBlock.class.getDeclaredField("lastExecution");lastExecution.setAccessible(true);
        updateLastExecution=net.minecraft.world.level.BaseCommandBlock.class.getDeclaredField("updateLastExecution");updateLastExecution.setAccessible(true);
        conditionMet=CommandBlockEntity.class.getDeclaredField("conditionMet");conditionMet.setAccessible(true);
        var out=new StringBuilder();
        for(String line:Files.readAllLines(Path.of(args[0]))) {
            if(line.isBlank()) continue;
            var fields=line.split("\\|",-1);
            if(fields.length!=11) throw new IllegalArgumentException("wrong fixture field count: "+line);
            var entity=fresh();var command=entity.getCommandBlock();
            command.setCommand(fields[0].equals("~") ? "" : fields[0]);
            command.setSuccessCount(Integer.parseInt(fields[1]));
            if(!fields[2].equals("~"))command.setCustomName(fields[2].startsWith("@red:")
                ?Component.literal(fields[2].substring(5)).withStyle(ChatFormatting.RED)
                :Component.literal(fields[2]));
            command.setTrackOutput(Boolean.parseBoolean(fields[3]));
            if(!fields[4].equals("~"))command.setLastOutput(fields[4].startsWith("@red:")
                ?Component.literal(fields[4].substring(5)).withStyle(ChatFormatting.RED)
                :Component.literal(fields[4]));
            updateLastExecution.setBoolean(command,Boolean.parseBoolean(fields[5]));
            lastExecution.setLong(command,Long.parseLong(fields[6]));
            entity.setPowered(Boolean.parseBoolean(fields[7]));
            entity.setAutomatic(Boolean.parseBoolean(fields[8]));
            conditionMet.setBoolean(entity,Boolean.parseBoolean(fields[9]));
            boolean full=Boolean.parseBoolean(fields[10]);
            CompoundTag saved=full?entity.saveWithFullMetadata(ParityData.lookup):entity.saveCustomOnly(ParityData.lookup);
            // Round-trip using the original Java 26.3 BlockEntity loader.
            var restored=net.minecraft.world.level.block.entity.BlockEntity.loadStatic(POS,
                Blocks.COMMAND_BLOCK.defaultBlockState(),entity.saveWithFullMetadata(ParityData.lookup),ParityData.lookup);
            if (restored==null) throw new IllegalStateException("vanilla loader rejected command block");
            CompoundTag resaved=full?restored.saveWithFullMetadata(ParityData.lookup):restored.saveCustomOnly(ParityData.lookup);
            out.append(inspect(saved)).append('#').append(inspect(resaved)).append('\n');
        }
        Files.writeString(Path.of(args[1]),out.toString());
    }
}
