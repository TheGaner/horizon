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
    turns_north: bool,
}

struct Case {
    block: CommandBlock,
    mode: Mode,
    conditional: bool,
    behind: Option<i32>,
    chain: Vec<Chain>,
    max_chain: usize,
    last_return: Option<bool>,
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
            chain: Vec::new(),
            max_chain: 65536,
            last_return: None,
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
        self.chain = if present {
            vec![Chain {
                conditional,
                powered,
                auto,
                ..Chain::default()
            }]
        } else {
            Vec::new()
        };
    }
    fn change_path(
        &mut self,
        count: usize,
        limit: usize,
        conditions: &str,
        active: &str,
        turn_at: usize,
    ) {
        assert!((1..=8).contains(&count));
        assert_eq!(conditions.len(), count);
        assert_eq!(active.len(), count * 2);
        assert!(
            conditions
                .bytes()
                .chain(active.bytes())
                .all(|b| b == b'0' || b == b'1')
        );
        self.max_chain = limit;
        self.chain = (0..count)
            .map(|idx| Chain {
                conditional: conditions.as_bytes()[idx] == b'1',
                powered: active.as_bytes()[idx * 2] == b'1',
                auto: active.as_bytes()[idx * 2 + 1] == b'1',
                turns_north: turn_at == idx + 1,
                ..Chain::default()
            })
            .collect();
    }
    fn invoke_root(&mut self) -> bool {
        if self.block.last_execution == self.time {
            return false;
        }
        if self.block.command.eq_ignore_ascii_case("Searge") {
            self.block.success_count = 1;
            self.block.last_output = Some(horizon_client::text::Text::literal("#itzlipofutzli"));
            return true;
        }
        self.block.success_count = 0;
        self.block.last_execution = if self.block.update_last_execution {
            self.time
        } else {
            -1
        };
        true
    }
    fn execute_chain(&mut self) {
        for index in 0..self.chain.len().min(self.max_chain) {
            let preceding_success = if index == 0 {
                self.block.success_count
            } else {
                self.chain[index - 1].success
            };
            let chain = &mut self.chain[index];
            if chain.powered || chain.auto {
                // Vanilla resolves the predecessor opposite *this node's*
                // facing; a north-turning node has no south predecessor in
                // our east-only test world, even when the west block succeeded.
                chain.condition_met =
                    !chain.conditional || (!chain.turns_north && preceding_success > 0);
                if chain.condition_met {
                    // Real Java BaseCommandBlock handles Searge without Brigadier.
                    chain.success = 1;
                    chain.last_output = "#itzlipofutzli".to_owned();
                    self.output_updates += 1;
                } else if chain.conditional {
                    chain.success = 0;
                }
            }
            if chain.turns_north {
                break;
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
                    if self.block.command.is_empty() {
                        self.block.success_count = 0;
                    } else {
                        self.invoke_root();
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
        let chain_state = if self.chain.is_empty() {
            "-".to_owned()
        } else {
            self.chain
                .iter()
                .map(|c| {
                    format!(
                        "{}:{}:{}:{}:{}:{}",
                        c.conditional, c.powered, c.auto, c.condition_met, c.success, c.last_output
                    )
                })
                .collect::<Vec<_>>()
                .join(";")
        };
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
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
            chain_state,
            self.block.last_execution,
            self.last_return
                .map_or_else(|| "-".to_owned(), |v| v.to_string())
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
            "command" => {
                state.block.set_command(match w[1] {
                    "empty" => "",
                    "searge" => "Searge",
                    "noop" => "noop",
                    _ => panic!("unknown command kind"),
                });
                // Vanilla CommandBlockEntity.setCommand marks the BE changed,
                // which notifies the neighboring comparator as well.
                state.output_updates += 1;
            }
            "invoke" => state.last_return = Some(state.invoke_root()),
            "signal" => state.signal(boolean(w[1])),
            "auto" => state.set_auto(boolean(w[1])),
            "mode" => state.set_mode(mode(w[1])),
            "conditional" => state.conditional = boolean(w[1]),
            "back" => state.behind = boolean(w[1]).then(|| w[2].parse::<i32>().unwrap()),
            "chain" => {
                state.change_chain(boolean(w[1]), boolean(w[2]), boolean(w[3]), boolean(w[4]))
            }
            "path" => state.change_path(
                w[1].parse().unwrap(),
                w[2].parse().unwrap(),
                w[3],
                w[4],
                w[5].parse().unwrap(),
            ),
            "success" => state.block.success_count = w[1].parse().unwrap(),
            "tick" => state.tick(),
            _ => panic!("unsupported: {line}"),
        }
        out.push_str(&state.snapshot());
        out.push('\n');
    }
    fs::write(&argv[2], out).expect("write output");
}
