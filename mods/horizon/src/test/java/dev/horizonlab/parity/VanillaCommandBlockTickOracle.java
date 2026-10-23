package dev.horizonlab.parity;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.RandomSource;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.CommandBlock;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.entity.CommandBlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.ticks.LevelTicks;
import net.minecraft.world.ticks.ScheduledTick;

/**
 * CommandBlock.tick, neighborChanged, onModeSwitch, setAutomatic and
 * markConditionMet against the genuine pinned Minecraft bytecode.
 *
 * The event clock/level are explicit stubs: we do not claim an integrated
 * real-world scheduling, command parsing, save/load or network test here.
 * The scheduled tick's *handler* is the real protected CommandBlock.tick.
 */
public final class VanillaCommandBlockTickOracle {
    static final BlockPos ROOT = new BlockPos(0, 64, 0);
    static final BlockPos WEST = new BlockPos(-1, 64, 0);
    static final BlockPos EAST = new BlockPos(1,64,0);
    static Method tick, neighbor;
    static Field lastExecution;
    static Object unsafe;
    static Method allocate;

    static class TickQueue extends LevelTicks<Block> {
        final List<ScheduledTick<Block>> pending = new ArrayList<>();
        TickQueue() { super(chunk -> true); }
        @Override public void schedule(ScheduledTick<Block> item) {
            for (var old : pending) {
                if (old.type() == item.type() && old.pos().equals(item.pos())) return;
            }
            pending.add(item);
        }
    }

    static class World extends ServerLevel {
        Map<BlockPos, BlockState> states;
        Map<BlockPos, BlockEntity> entities;
        TickQueue queue;
        GameRules rules;
        long clock, subTick;
        boolean inputSignal;
        int updates;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public BlockState getBlockState(BlockPos p) { return states.getOrDefault(p, Blocks.AIR.defaultBlockState()); }
        @Override public BlockEntity getBlockEntity(BlockPos p) { return entities.get(p); }
        @Override public boolean hasNeighborSignal(BlockPos p) { return inputSignal && p.equals(ROOT); }
        @Override public LevelTicks<Block> getBlockTicks() { return queue; }
        @Override public GameRules getGameRules() { return rules; }
        // Test nonempty commands without invoking a nonexistent full server dispatcher.
        @Override public boolean isCommandBlockEnabled() { return false; }
        @Override public long getGameTime() { return clock; }
        @Override public long nextSubTickCount() { return subTick++; }
        @Override public void updateNeighbourForOutputSignal(BlockPos p, Block source) { updates++; }
        @Override public void blockEntityChanged(BlockPos p) { /* no actual chunks in this stub */ }
    }

    static World world;
    static CommandBlockEntity root, behind, chain;
    static Boolean lastReturn;
    static List<CommandBlockEntity> chainPath = List.of();

    static <T> T alloc(Class<T> cls) throws Exception { return cls.cast(allocate.invoke(unsafe, cls)); }
    static BlockState block(String mode, boolean conditional) {
        Block b = switch(mode) {
            case "impulse" -> Blocks.COMMAND_BLOCK;
            case "repeat" -> Blocks.REPEATING_COMMAND_BLOCK;
            case "chain" -> Blocks.CHAIN_COMMAND_BLOCK;
            default -> throw new IllegalArgumentException(mode);
        };
        return b.defaultBlockState().setValue(CommandBlock.FACING, Direction.EAST)
            .setValue(CommandBlock.CONDITIONAL, conditional);
    }
    static void reset(String mode, boolean cond, boolean auto, boolean hasBehind, int behindSuccess, boolean searge) throws Exception {
        world = alloc(World.class);
        world.states = new HashMap<>();
        world.entities = new HashMap<>();
        world.queue = new TickQueue();
        world.rules = new GameRules(FeatureFlags.DEFAULT_FLAGS);
        world.clock=0;world.subTick=0;world.updates=0;world.inputSignal=false;
        lastReturn=null;
        var state=block(mode,cond);
        world.states.put(ROOT,state);
        root=new CommandBlockEntity(ROOT,state);
        if (searge) root.getCommandBlock().setCommand("Searge");
        root.setAutomatic(auto); // placement/default entity before Level assigned
        root.setLevel(world);
        world.entities.put(ROOT,root);
        changeBehind(hasBehind, behindSuccess);
        chain=null;
        chainPath=List.of();
    }
    static void clearPath() {
        for (int i=1;i<=8;i++) {
            var pos=ROOT.offset(i,0,0);
            world.entities.remove(pos);world.states.remove(pos);
        }
        chain=null;chainPath=List.of();
    }
    static void changeChain(boolean present,boolean conditional,boolean powered,boolean auto) {
        clearPath();
        if (!present) return;
        createPath(1, conditional ? "1" : "0", (powered ? "1" : "0")+(auto ? "1" : "0"), 0);
    }
    static void createPath(int count,String conditionalMask,String activationMask,int turnAt) {
        if (count<0 || count>8 || conditionalMask.length()!=count || activationMask.length()!=count*2) {
            throw new IllegalArgumentException("bad path specification");
        }
        clearPath();
        var all=new ArrayList<CommandBlockEntity>();
        for (int i=1;i<=count;i++) {
            var pos=ROOT.offset(i,0,0);
            var state=block("chain",conditionalMask.charAt(i-1)=='1');
            if (i==turnAt) state=state.setValue(CommandBlock.FACING,Direction.NORTH);
            world.states.put(pos,state);
            var entity=new CommandBlockEntity(pos,state);
            entity.getCommandBlock().setCommand("Searge");
            entity.setAutomatic(activationMask.charAt((i-1)*2+1)=='1');
            entity.setPowered(activationMask.charAt((i-1)*2)=='1');
            entity.setLevel(world);
            world.entities.put(pos,entity);
            all.add(entity);
        }
        chainPath=List.copyOf(all);
        chain=all.isEmpty()?null:all.get(0);
    }
    static void configurePath(int count,int limit,String condMask,String activeMask,int turnAt) {
        world.rules.set(GameRules.MAX_COMMAND_SEQUENCE_LENGTH,limit,null);
        createPath(count,condMask,activeMask,turnAt);
    }
    static void changeBehind(boolean present,int success) {
        world.entities.remove(WEST);
        if(present) {
            var state=block("impulse",false);
            world.states.put(WEST,state);
            behind=new CommandBlockEntity(WEST,state);
            behind.getCommandBlock().setSuccessCount(success);
            behind.setLevel(world);
            world.entities.put(WEST,behind);
        } else {world.states.remove(WEST);behind=null;}
    }
    static void signal(boolean value) throws Exception {
        world.inputSignal=value;
        var state=world.getBlockState(ROOT);
        neighbor.invoke(state.getBlock(),state,world,ROOT,state.getBlock(),null,false);
    }
    static void advance() throws Exception {
        world.clock++;
        var due = new ArrayList<ScheduledTick<Block>>();
        for (var item : world.queue.pending) if (item.triggerTick()<=world.clock) due.add(item);
        world.queue.pending.removeAll(due);
        for (var item : due) {
            var state=world.getBlockState(item.pos());
            if (state.getBlock()==item.type())
                tick.invoke(state.getBlock(),state,world,item.pos(),RandomSource.create(0));
        }
    }
    static String snapshot() {
        StringBuilder q=new StringBuilder();
        for(var item:world.queue.pending) {
            if(q.length()>0)q.append(',');
            q.append(item.triggerTick()).append(':').append(item.type()==Blocks.COMMAND_BLOCK?'I':
              item.type()==Blocks.REPEATING_COMMAND_BLOCK?'R':'C');
        }
        var cb=root.getCommandBlock();
        String chainState="-";
        if (!chainPath.isEmpty()) {
            var entries=new ArrayList<String>();
            for (var entity:chainPath) {
                var cc=entity.getCommandBlock();
                entries.add(world.getBlockState(entity.getBlockPos()).getValue(CommandBlock.CONDITIONAL)+":"+
                    entity.isPowered()+":"+entity.isAutomatic()+":"+entity.wasConditionMet()+":"+
                    cc.getSuccessCount()+":"+cc.getLastOutput().getString());
            }
            chainState=String.join(";",entries);
        }
        long executed;
        try {executed=lastExecution.getLong(cb);} catch (IllegalAccessException e) {throw new IllegalStateException(e);}
        return world.clock+"|"+root.getMode()+"|"+world.getBlockState(ROOT).getValue(CommandBlock.CONDITIONAL)+
            "|"+root.isPowered()+"|"+root.isAutomatic()+"|"+root.wasConditionMet()+"|"+
            cb.getSuccessCount()+"|"+world.updates+"|"+q+"|"+cb.getCommand()+"|"+cb.getLastOutput().getString()+"|"+chainState+"|"+executed+"|"+(lastReturn==null?"-":lastReturn.toString());
    }
    public static void main(String[] args) throws Exception {
        ParityData.init();
        var f=Class.forName("sun.misc.Unsafe").getDeclaredField("theUnsafe");
        f.setAccessible(true);unsafe=f.get(null);allocate=unsafe.getClass().getMethod("allocateInstance",Class.class);
        neighbor=CommandBlock.class.getDeclaredMethod("neighborChanged",BlockState.class,Level.class,BlockPos.class,Block.class,
          net.minecraft.world.level.redstone.Orientation.class,boolean.class);
        neighbor.setAccessible(true);
        tick=CommandBlock.class.getDeclaredMethod("tick",BlockState.class,ServerLevel.class,BlockPos.class,RandomSource.class);
        tick.setAccessible(true);
        lastExecution=net.minecraft.world.level.BaseCommandBlock.class.getDeclaredField("lastExecution");
        lastExecution.setAccessible(true);
        var out=new StringBuilder();
        for(String line:Files.readAllLines(Path.of(args[0]))) {
            String[] w=line.split(" ");
            switch(w[0]) {
                case "reset" -> reset(w[1],Boolean.parseBoolean(w[2]),Boolean.parseBoolean(w[3]),Boolean.parseBoolean(w[4]),Integer.parseInt(w[5]),w.length>6 && w[6].equals("searge"));
                case "command" -> root.getCommandBlock().setCommand(w[1].equals("empty") ? "" : w[1].equals("searge") ? "Searge" : "noop");
                case "invoke" -> lastReturn=root.getCommandBlock().performCommand(world);
                case "signal" -> signal(Boolean.parseBoolean(w[1]));
                case "auto" -> root.setAutomatic(Boolean.parseBoolean(w[1]));
                case "mode" -> {
                    var state=block(w[1],world.getBlockState(ROOT).getValue(CommandBlock.CONDITIONAL));
                    world.states.put(ROOT,state); root.setBlockState(state);root.onModeSwitch();
                }
                case "conditional" -> {
                    var state=world.getBlockState(ROOT).setValue(CommandBlock.CONDITIONAL,Boolean.parseBoolean(w[1]));
                    world.states.put(ROOT,state);root.setBlockState(state);
                }
                case "back" -> changeBehind(Boolean.parseBoolean(w[1]),Integer.parseInt(w[2]));
                case "chain" -> changeChain(Boolean.parseBoolean(w[1]),Boolean.parseBoolean(w[2]),Boolean.parseBoolean(w[3]),Boolean.parseBoolean(w[4]));
                case "path" -> configurePath(Integer.parseInt(w[1]),Integer.parseInt(w[2]),w[3],w[4],Integer.parseInt(w[5]));
                case "success" -> root.getCommandBlock().setSuccessCount(Integer.parseInt(w[1]));
                case "tick" -> advance();
                default -> throw new IllegalArgumentException(line);
            }
            out.append(snapshot()).append('\n');
        }
        Files.writeString(Path.of(args[1]),out.toString());
    }
}
