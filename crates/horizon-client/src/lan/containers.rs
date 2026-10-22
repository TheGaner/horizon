//! Hub operations on what guests perceive and do beyond the basics: sounds,
//! level events, animations and block containers with their clicks.
use super::{
    hub::Hub,
    menu::{self, Menu, Rules, Slots, View},
};
use crate::{containers::BlockEntity, items::ItemStack, world::World};
use horizon_protocol::{
    item::Hashed,
    packets::play::{self, Clientbound as C},
};
use horizon_world::nbt::Tag;
use std::collections::HashMap;

impl Hub {
    pub fn send_to(&self, conn: u32, packet: &C) {
        if let Some(g) = self.guest(conn) {
            g.send(packet);
        }
    }

    /// Send to every placed guest within `range` blocks of `at`.
    pub fn broadcast_near(&self, at: [f64; 3], range: f64, packet: &C, except: Option<u32>) {
        let body = packet.encode();
        for g in self
            .guests
            .iter()
            .filter(|g| self.here(g) && Some(g.conn) != except)
        {
            let d: f64 = (0..3).map(|k| (g.position[k] - at[k]).powi(2)).sum();
            if d <= range * range {
                g.sender.send(body.clone());
            }
        }
    }

    /// ClientboundSoundPacket for a sound event name (an inline event when
    /// the registry does not know it).
    pub fn sound(
        &self,
        name: &str,
        at: [f64; 3],
        volume: f32,
        pitch: f32,
        source: i32,
        except: Option<u32>,
    ) {
        let sound = match self.sync.id("minecraft:sound_event", name) {
            Some(id) => (id, None),
            None => (-1, Some((name.to_owned(), None))),
        };
        let packet = C::Sound(play::Sound {
            sound,
            source,
            x: (at[0] * 8.0) as i32,
            y: (at[1] * 8.0) as i32,
            z: (at[2] * 8.0) as i32,
            volume,
            pitch,
            seed: self.ticks as i64,
        });
        self.broadcast_near(at, f64::from(volume.max(1.0)) * 16.0, &packet, except);
    }

    /// ClientboundLevelEventPacket (block break particles and sounds ...).
    pub fn level_event(&self, kind: i32, pos: [i32; 3], data: i32, except: Option<u32>) {
        let at = [f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2])];
        self.broadcast_near(
            at,
            64.0,
            &C::LevelEvent(play::LevelEvent {
                kind,
                pos,
                data,
                global: false,
            }),
            except,
        );
    }

    /// ClientboundBlockEventPacket (`PlayerList.broadcast` within 64 blocks).
    pub fn block_event(&self, pos: [i32; 3], b0: i32, b1: i32, state: u32) {
        let block = self
            .encoder
            .block_ids
            .get(state as usize)
            .copied()
            .unwrap_or(0);
        let at = [f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2])];
        self.broadcast_near(
            at,
            64.0,
            &C::BlockEvent(play::BlockEvent {
                pos,
                b0: b0 as u8,
                b1: b1 as u8,
                block,
            }),
            None,
        );
    }

    pub fn entity_event(&self, id: i32, event: i8, at: [f64; 3]) {
        self.broadcast_near(
            at,
            128.0,
            &C::EntityEvent(play::EntityEvent { id, event }),
            None,
        );
    }

    /// `ServerLevel.broadcastDamageEvent`: the clients around show the hurt
    /// entity's flash and hear its hurt sound.
    pub fn damage_event(
        &self,
        id: i32,
        kind: &str,
        cause: Option<i32>,
        direct: Option<i32>,
        at: [f64; 3],
    ) {
        let Some(source_type) = self.sync.id("minecraft:damage_type", kind) else {
            return;
        };
        self.broadcast_near(
            at,
            128.0,
            &C::DamageEvent(play::DamageEvent {
                id,
                source_type,
                cause,
                direct,
                position: None,
            }),
            None,
        );
    }

    pub fn swing(&self, id: i32, hand: i32, at: [f64; 3], except: Option<u32>) {
        self.broadcast_near(
            at,
            128.0,
            &C::SwingAnimation(play::SwingAnimation {
                id,
                hand,
                kind: 1,
                duration: 6,
            }),
            except,
        );
    }

    pub fn take_item(&self, item: i32, collector: i32, count: i32, at: [f64; 3]) {
        self.broadcast_near(
            at,
            64.0,
            &C::TakeItemEntity(play::TakeItemEntity {
                item,
                player: collector,
                amount: count,
            }),
            None,
        );
    }

    /// Open a block container for a guest (ServerPlayer.openMenu).
    pub fn open_container(
        &mut self,
        conn: u32,
        menu_kind: &str,
        title: Tag,
        pos: (i32, i32, i32),
        slots: Slots,
        items: Vec<Option<ItemStack>>,
    ) {
        let Some(kind) = self
            .sync
            .builtin
            .get("minecraft:menu")
            .and_then(|m| m.id(menu_kind))
        else {
            return;
        };
        let Some(i) = self.guests.iter().position(|g| g.conn == conn) else {
            return;
        };
        self.close_container_at(i);
        let g = &mut self.guests[i];
        g.next_window = g.next_window % 100 + 1;
        let window = g.next_window;
        g.send(&C::OpenScreen(play::OpenScreen {
            container: window,
            menu: kind,
            title,
        }));
        g.menu = Some(Menu {
            window,
            kind,
            pos: Some(pos),
            slots,
            size: items.len(),
            state: 0,
            sent: Vec::new(),
            drag: None,
            sent_data: Vec::new(),
        });
        self.send_menu_content(i, items);
    }

    /// Open a trading window for a guest (`Merchant.openTradingScreen`):
    /// its id, or None when the menu type is unknown.
    pub fn open_merchant_window(&mut self, conn: u32, title: Tag) -> Option<i32> {
        self.open_window(conn, "minecraft:merchant", title)
    }

    /// ServerPlayer.openMenu for a menu the game runs (merchant, crafting
    /// table): the open container closes, a new window id opens this menu.
    pub fn open_window(&mut self, conn: u32, menu: &str, title: Tag) -> Option<i32> {
        let kind = self
            .sync
            .builtin
            .get("minecraft:menu")
            .and_then(|m| m.id(menu))?;
        let i = self.guests.iter().position(|g| g.conn == conn)?;
        self.close_container_at(i);
        let g = &mut self.guests[i];
        g.next_window = g.next_window % 100 + 1;
        let window = g.next_window;
        g.send(&C::OpenScreen(play::OpenScreen {
            container: window,
            menu: kind,
            title,
        }));
        Some(window)
    }

    fn send_menu_content(&mut self, i: usize, mut items: Vec<Option<ItemStack>>) {
        for j in 0..36 {
            items.push(self.guests[i].inventory[player_slot(j)].clone());
        }
        let net: Vec<_> = items.iter().map(|s| self.stack(s)).collect();
        let carried = self.guests[i].carried.clone();
        let carried = self.stack(&carried);
        let g = &mut self.guests[i];
        let Some(menu) = g.menu.as_mut() else {
            return;
        };
        menu.state = (menu.state + 1) & 0x7FFF;
        let packet = C::ContainerSetContent(play::ContainerSetContent {
            container: menu.window,
            state: menu.state,
            items: net,
            carried,
        });
        menu.sent = items;
        g.send(&packet);
    }

    fn close_container_at(&mut self, i: usize) {
        let g = &mut self.guests[i];
        if let Some(menu) = g.menu.take() {
            g.send(&C::ContainerClose(play::ContainerClose {
                container: menu.window,
            }));
        }
    }

    pub(super) fn close_container_for(&mut self, i: usize) {
        self.close_container_at(i);
    }

    pub fn close_container(&mut self, conn: u32) {
        if let Some(i) = self.guests.iter().position(|g| g.conn == conn) {
            self.close_container_at(i);
        }
    }

    /// The client closed its window.
    pub fn container_closed(&mut self, conn: u32, window: i32) {
        if let Some(g) = self.guest_mut(conn)
            && g.menu.as_ref().is_some_and(|m| m.window == window)
        {
            g.menu = None;
        }
    }

    fn hashed_matches(&mut self, claimed: &Option<Hashed>, mine: &Option<ItemStack>) -> bool {
        match (claimed, mine) {
            (None, None) => true,
            (Some(h), Some(s)) => h.item == self.item_id(&s.item) && h.count == s.count as i32,
            _ => false,
        }
    }

    fn view_slot(&self, i: usize, world: &World, slot: usize) -> Option<ItemStack> {
        let g = &self.guests[i];
        match &g.menu {
            Some(m) if slot < m.size => m.slots.get(world, g, slot),
            Some(m) => g
                .inventory
                .get(player_slot(slot - m.size))
                .cloned()
                .flatten(),
            None => g.menu_slot(slot),
        }
    }

    /// ServerGamePacketListenerImpl.handleContainerClick. Returns the stacks
    /// thrown out of the menu.
    pub fn container_click(
        &mut self,
        conn: u32,
        p: &play::ContainerClick,
        world: &mut World,
        rules: &dyn Rules,
    ) -> Vec<ItemStack> {
        let Some(i) = self.guests.iter().position(|g| g.conn == conn) else {
            return Vec::new();
        };
        let in_menu = p.container != 0;
        let window_ok = !in_menu
            || self.guests[i]
                .menu
                .as_ref()
                .is_some_and(|m| m.window == p.container);
        if !window_ok || self.guests[i].spectator() {
            return Vec::new();
        }
        let source = self.guests[i].menu.as_ref().map(|m| m.slots);
        let mut drag = if in_menu {
            self.guests[i].menu.as_mut().and_then(|m| m.drag.take())
        } else {
            self.guests[i].drag.take()
        };
        let guest = &mut self.guests[i];
        let mut items = source.and_then(|s| s.read(world, guest));
        if in_menu && items.is_none() {
            return Vec::new();
        }
        let mut view = View {
            guest,
            container: items.as_deref_mut(),
        };
        let (drops, changed) = menu::click(
            &mut view,
            &mut drag,
            i32::from(p.slot),
            i32::from(p.button),
            p.input,
            rules,
        );
        if let (Some(source), Some(items)) = (source, items) {
            source.write(world, guest, items);
        }
        if in_menu {
            if let Some(m) = self.guests[i].menu.as_mut() {
                m.drag = drag;
            }
        } else {
            self.guests[i].drag = drag;
        }
        self.answer_click(i, p, world, &changed);
        drops
    }

    /// Java 26.3 InventoryMenu THROW: after running the action, the *real*
    /// client already holds the predicted changedSlots and carried stack.
    /// The item entity is paired on the next tracker tick; an incorrect or
    /// empty prediction must still be corrected after that AddEntity packet.
    /// Only mark the precisely claimed changed slots as sent, never unrelated
    /// pending inventory changes awaiting normal sync_player_state.
    pub fn accept_predicted_inventory_throw(
        &mut self,
        conn: u32,
        p: &play::ContainerClick,
        world: &World,
        changed: &[usize],
    ) -> bool {
        let Some(i) = self.guests.iter().position(|g| g.conn == conn) else {
            return false;
        };
        if p.container != 0
            || p.input != 4
            || p.state != self.guests[i].menu_state
            || !exactly_claims_changed_slots(changed, &p.changed)
        {
            return false;
        }
        for &(slot, ref claim) in &p.changed {
            let slot = slot as usize;
            let actual = self.view_slot(i, world, slot);
            // A custom data patch hash cannot be authenticated with the
            // current registry-less comparator; let the normal correction
            // run rather than accepting unverified component claims.
            if claim
                .as_ref()
                .is_some_and(|h| !h.added.is_empty() || !h.removed.is_empty())
                || !plain_predicted_stack(&actual)
                || !self.hashed_matches(claim, &actual)
            {
                return false;
            }
        }
        let carried = self.guests[i].carried.clone();
        if !plain_predicted_stack(&carried) || !self.hashed_matches(&p.carried, &carried) {
            return false;
        }
        let updates: Vec<_> = changed
            .iter()
            .map(|&s| (s, self.view_slot(i, world, s)))
            .collect();
        for (slot, actual) in updates {
            self.guests[i].menu_sent[slot] = actual;
        }
        true
    }

    /// A click on the inventory menu (window 0) the game applied itself
    /// (the crafting grid): the client is corrected like after any click.
    pub fn inventory_clicked(
        &mut self,
        conn: u32,
        p: &play::ContainerClick,
        world: &World,
        changed: &[usize],
    ) {
        if let Some(i) = self.guests.iter().position(|g| g.conn == conn) {
            self.answer_click(i, p, world, changed);
        }
    }

    /// After a click: tell the client about every slot it got wrong, or that
    /// it did not claim, and the carried stack.
    fn answer_click(
        &mut self,
        i: usize,
        p: &play::ContainerClick,
        world: &World,
        changed: &[usize],
    ) {
        let in_menu = p.container != 0;
        let claimed: HashMap<usize, Option<Hashed>> = p
            .changed
            .iter()
            .map(|(s, h)| (*s as usize, h.clone()))
            .collect();
        let total = if in_menu {
            self.guests[i].menu.as_ref().map_or(0, |m| m.size) + 36
        } else {
            46
        };
        let mut resend: Vec<usize> = Vec::new();
        for slot in 0..total {
            let mine = self.view_slot(i, world, slot);
            let agreed = match claimed.get(&slot) {
                Some(c) => self.hashed_matches(c, &mine),
                None => !changed.contains(&slot),
            };
            if !agreed {
                resend.push(slot);
            }
        }
        for slot in resend {
            let mine = self.view_slot(i, world, slot);
            let net = self.stack(&mine);
            let g = &mut self.guests[i];
            let state = if in_menu {
                g.menu.as_mut().map_or(0, |m| {
                    m.state = (m.state + 1) & 0x7FFF;
                    m.state
                })
            } else {
                g.menu_state = (g.menu_state + 1) & 0x7FFF;
                g.menu_state
            };
            g.send(&C::ContainerSetSlot(play::ContainerSetSlot {
                container: p.container,
                state,
                slot: slot as i16,
                item: net,
            }));
        }
        let carried = self.guests[i].carried.clone();
        if !self.hashed_matches(&p.carried, &carried) {
            let net = self.stack(&carried);
            self.guests[i].send(&C::SetCursorItem(play::SetCursorItem { item: net }));
        }
        // What the client now believes equals the server's state.
        let items: Vec<_> = (0..total).map(|s| self.view_slot(i, world, s)).collect();
        if in_menu {
            if let Some(m) = self.guests[i].menu.as_mut() {
                m.sent = items;
            }
        } else {
            self.guests[i].menu_sent = items;
        }
    }

    /// AbstractContainerMenu.broadcastChanges' data slots: a furnace's
    /// AbstractFurnaceBlockEntity.dataAccess (lit time left, lit total, cooking
    /// timer, cooking total), each sent when it changes.
    fn sync_container_data(&mut self, i: usize, world: &World) {
        let g = &mut self.guests[i];
        let Some(m) = g.menu.as_mut() else {
            return;
        };
        let Slots::Block(pos) = m.slots else {
            return;
        };
        // AbstractFurnaceBlockEntity.dataAccess, BrewingStandBlockEntity.dataAccess.
        let data = match world.block_entities.get(&pos) {
            Some(BlockEntity::Furnace(f)) => [
                f.lit_remaining,
                f.lit_total,
                f.cooking_timer,
                f.cooking_total,
            ],
            Some(BlockEntity::Brewing(b)) => [b.brew_time, b.fuel, b.total_brew_time, b.total_fuel],
            _ => return,
        }
        .map(|v| v as i16);
        m.sent_data.resize(data.len(), i16::MIN);
        let window = m.window;
        let changed: Vec<(usize, i16)> = data
            .iter()
            .enumerate()
            .filter(|&(k, v)| m.sent_data[k] != *v)
            .map(|(k, &v)| (k, v))
            .collect();
        for &(k, value) in &changed {
            m.sent_data[k] = value;
        }
        for (k, value) in changed {
            g.send(&C::ContainerSetData(play::ContainerSetData {
                container: window,
                id: k as i16,
                value,
            }));
        }
    }

    /// Slot changes of open containers (AbstractContainerMenu.broadcastChanges).
    pub fn sync_containers(&mut self, world: &World) {
        for i in 0..self.guests.len() {
            let Some(menu) = self.guests[i]
                .menu
                .as_ref()
                .filter(|_| self.here(&self.guests[i]))
            else {
                continue;
            };
            let (size, window, slots) = (menu.size, menu.window, menu.slots);
            if !slots.valid(world) {
                self.close_container_at(i);
                continue;
            }
            for slot in 0..size + 36 {
                let now = self.view_slot(i, world, slot);
                let sent = self.guests[i]
                    .menu
                    .as_ref()
                    .and_then(|m| m.sent.get(slot).cloned());
                if sent.as_ref() == Some(&now) {
                    continue;
                }
                let net = self.stack(&now);
                let g = &mut self.guests[i];
                if let Some(m) = g.menu.as_mut() {
                    m.state = (m.state + 1) & 0x7FFF;
                    let state = m.state;
                    if m.sent.len() <= slot {
                        m.sent.resize(slot + 1, None);
                    }
                    m.sent[slot] = now;
                    g.send(&C::ContainerSetSlot(play::ContainerSetSlot {
                        container: window,
                        state,
                        slot: slot as i16,
                        item: net,
                    }));
                }
            }
            // Then the data slots, as broadcastChanges does.
            self.sync_container_data(i, world);
        }
    }
}

/// HashedStack's component patch requires vanilla's decorated registry hash
/// generator, which this Hub does not model yet. Even a client that CLAIMS an
/// empty patch cannot be trusted when the authoritative stack is enchanted,
/// damaged or carries other NBT components; prefer corrective packets instead.
fn plain_predicted_stack(stack: &Option<ItemStack>) -> bool {
    stack
        .as_ref()
        .is_none_or(|s| s.damage == 0 && s.extra.is_none())
}

/// A predicted menu click must supply one distinct claim for every changed
/// slot. Missing, duplicate or unrelated claims must retain correction path.
fn exactly_claims_changed_slots(changed: &[usize], claims: &[(i16, Option<Hashed>)]) -> bool {
    !changed.is_empty()
        && changed.len() == claims.len()
        && changed.iter().all(|&index| {
            claims
                .iter()
                .filter(|(slot, _)| *slot == index as i16)
                .count()
                == 1
        })
}

#[cfg(test)]
mod predicted_throw_claim_tests {
    use super::*;

    #[test]
    fn component_bearing_items_are_never_accepted_by_count_only_hash_check() {
        assert!(plain_predicted_stack(&None));
        let plain = ItemStack {
            item: std::sync::Arc::from("minecraft:stick"),
            count: 2,
            damage: 0,
            extra: None,
        };
        assert!(plain_predicted_stack(&Some(plain.clone())));
        let mut damaged = plain.clone();
        damaged.damage = 1;
        assert!(!plain_predicted_stack(&Some(damaged)));
        let mut custom = plain;
        custom.extra = Some(Box::new(crate::item_data::StackData::default()));
        assert!(!plain_predicted_stack(&Some(custom)));
    }

    #[test]
    fn changed_slots_must_be_exact_before_suppressing_inventory_correction() {
        assert!(exactly_claims_changed_slots(&[36], &[(36, None)]));
        assert!(exactly_claims_changed_slots(
            &[36, 37],
            &[(37, None), (36, None)]
        ));
        assert!(!exactly_claims_changed_slots(&[36], &[]));
        assert!(!exactly_claims_changed_slots(&[36], &[(37, None)]));
        assert!(!exactly_claims_changed_slots(
            &[36],
            &[(36, None), (36, None)]
        ));
        assert!(!exactly_claims_changed_slots(
            &[36],
            &[(36, None), (37, None)]
        ));
        assert!(!exactly_claims_changed_slots(&[], &[]));
    }
}

/// Inventory index of the n-th player slot of a container menu (27 main, then the hotbar).
fn player_slot(n: usize) -> usize {
    if n < 27 { 9 + n } else { n - 27 }
}
