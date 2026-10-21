//! Rust-side mirror of the bounded multi-tick vanilla command block oracle.
//! The true Java CommandBlock/CommandBlockEntity methods are exercised by
//! VanillaCommandBlockTickOracle. This checks Horizon's production transition
//! functions with empty/Searge commands; no fake vanilla expected-output table.
use horizon_client::command_block::{self, CommandBlock, Mode};
use std::fs;

#[derive(Default)]
struct Chain {
    conditional: bool,
    powered: bool,
    auto: bool,
    condition_met: bool,
    success: i32,
    last_output: String,
}

struct Case {
    block: CommandBlock,
    mode: Mode,
    conditional: bool,
    behind: Option<i32>,
    chain: Option<Chain>,
    time: i64,
    scheduled: Vec<(i64, Mode)>,
    output_updates: usize,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            block: CommandBlock::default(),
            mode: Mode::Redstone,
            conditional: false,
            behind: None,
            chain: None,
            time: 0,
            scheduled: Vec::new(),
            output_updates: 0,
        }
    }
}

impl Case {
    fn condition(&mut self) {
        self.block.condition_met = !self.conditional || self.behind.is_some_and(|n| n > 0);
    }
    fn schedule(&mut self) {
        if self.scheduled.iter().any(|(_, m)| *m == self.mode) {
            return;
        }
        self.scheduled.push((self.time + 1, self.mode));
    }
    fn reset(
        &mut self,
        mode: Mode,
        conditional: bool,
        auto: bool,
        behind: Option<i32>,
        searge: bool,
    ) {
        *self = Self::default();
        self.mode = mode;
        self.conditional = conditional;
        self.block.auto = auto;
        self.behind = behind;
        if searge {
            self.block.set_command("Searge");
        }
    }
    fn signal(&mut self, powered: bool) {
        let before = self.block.powered;
        self.block.powered = powered;
        if command_block::power_schedules(self.mode, before, powered, self.block.auto) {
            self.condition();
            self.schedule();
        }
    }
    fn set_auto(&mut self, auto: bool) {
        let before = self.block.auto;
        self.block.auto = auto;
        if !before && auto && !self.block.powered && self.mode != Mode::Sequence {
            self.condition();
            self.schedule();
        }
    }
    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        if mode == Mode::Auto && (self.block.powered || self.block.auto) {
            self.condition();
            self.schedule();
        }
    }
    fn change_chain(&mut self, present: bool, conditional: bool, powered: bool, auto: bool) {
        self.chain = present.then_some(Chain {
            conditional,
            powered,
            auto,
            ..Chain::default()
        });
    }
    fn execute_chain(&mut self) {
        if let Some(chain) = self.chain.as_mut() {
            if chain.powered || chain.auto {
                chain.condition_met = !chain.conditional || self.block.success_count > 0;
                if chain.condition_met {
                    // Its real Java BaseCommandBlock executes Searge directly,
                    // without the MinecraftServer command dispatcher.
                    chain.success = 1;
                    chain.last_output = "#itzlipofutzli".to_owned();
                    self.output_updates += 1;
                } else if chain.conditional {
                    chain.success = 0;
                }
            }
        }
    }
    fn tick(&mut self) {
        self.time += 1;
        let mut due = Vec::new();
        self.scheduled.retain(|&(time, mode)| {
            if time <= self.time {
                due.push(mode);
                false
            } else {
                true
            }
        });
        for previous_mode in due {
            if previous_mode != self.mode {
                continue;
            }
            let was = self.block.condition_met;
            if self.mode == Mode::Auto {
                self.condition();
            }
            match command_block::tick_action(self.mode, was, self.conditional) {
                Some(true) => {
                    if self.block.command.eq_ignore_ascii_case("Searge") {
                        self.block.success_count = 1;
                        self.block.last_output =
                            Some(horizon_client::text::Text::literal("#itzlipofutzli"));
                    } else {
                        self.block.success_count = 0;
                    }
                    self.execute_chain();
                }
                Some(false) => self.block.success_count = 0,
                None => {}
            }
            if command_block::repeats(self.mode, self.block.powered, self.block.auto) {
                self.schedule();
            }
            self.output_updates += 1;
        }
    }
    fn snapshot(&self) -> String {
        let mut schedule = String::new();
        for (time, mode) in &self.scheduled {
            if !schedule.is_empty() {
                schedule.push(',');
            }
            schedule.push_str(&format!(
                "{time}:{}",
                match mode {
                    Mode::Redstone => "I",
                    Mode::Auto => "R",
                    Mode::Sequence => "C",
                }
            ));
        }
        let chain_state = self.chain.as_ref().map_or_else(
            || "-".to_owned(),
            |c| {
                format!(
                    "{}:{}:{}:{}:{}:{}",
                    c.conditional, c.powered, c.auto, c.condition_met, c.success, c.last_output
                )
            },
        );
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.time,
            match self.mode {
                Mode::Redstone => "REDSTONE",
                Mode::Auto => "AUTO",
                Mode::Sequence => "SEQUENCE",
            },
            self.conditional,
            self.block.powered,
            self.block.auto,
            self.block.condition_met,
            self.block.success_count,
            self.output_updates,
            schedule,
            self.block.command,
            self.block.last_output_string(),
            chain_state
        )
    }
}

fn mode(s: &str) -> Mode {
    match s {
        "impulse" => Mode::Redstone,
        "repeat" => Mode::Auto,
        "chain" => Mode::Sequence,
        _ => panic!("bad mode {s}"),
    }
}
fn boolean(s: &str) -> bool {
    match s {
        "true" => true,
        "false" => false,
        _ => panic!("bad boolean {s}"),
    }
}
fn main() {
    let argv: Vec<String> = std::env::args().collect();
    assert_eq!(
        argv.len(),
        3,
        "usage: command_block_tick_fixture INPUT OUTPUT"
    );
    let commands = fs::read_to_string(&argv[1]).expect("input");
    let mut state = Case::default();
    let mut out = String::new();
    for line in commands.lines() {
        let w: Vec<_> = line.split_whitespace().collect();
        match w[0] {
            "reset" => state.reset(
                mode(w[1]),
                boolean(w[2]),
                boolean(w[3]),
                boolean(w[4]).then(|| w[5].parse::<i32>().unwrap()),
                w.get(6) == Some(&"searge"),
            ),
            "signal" => state.signal(boolean(w[1])),
            "auto" => state.set_auto(boolean(w[1])),
            "mode" => state.set_mode(mode(w[1])),
            "conditional" => state.conditional = boolean(w[1]),
            "back" => state.behind = boolean(w[1]).then(|| w[2].parse::<i32>().unwrap()),
            "chain" => {
                state.change_chain(boolean(w[1]), boolean(w[2]), boolean(w[3]), boolean(w[4]))
            }
            "success" => state.block.success_count = w[1].parse().unwrap(),
            "tick" => state.tick(),
            _ => panic!("unsupported: {line}"),
        }
        out.push_str(&state.snapshot());
        out.push('\n');
    }
    fs::write(&argv[2], out).expect("write output");
}
