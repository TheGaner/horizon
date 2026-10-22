//! Non-player entities simulated on the client world. Dropped items follow
//! `ItemEntity.tick`: gravity 0.04, 0.98 drag (block friction on the ground),
//! a ground bounce of -0.5, water buoyancy, pickup delay and 6000-tick despawn.
use crate::{
    blocks::{BlockFacts, Fluid},
    collisions::SceneCollisions,
    items::ItemStack,
    world::World,
};
use horizon_core::geometry::{Aabb, Vec3};
use horizon_world::block_collision::BlockCollisionRegistry;

#[derive(Clone, Debug)]
pub struct ItemEntity {
    pub health: i32,
    pub fire_ticks: i32,
    pub id: u32,
    pub stack: ItemStack,
    pub thrower: Option<[i32; 4]>,
    /// Vanilla ItemEntity.target (`Owner` in NBT), not the dropper (`Thrower`).
    pub target: Option<[i32; 4]>,
    pub position: Vec3,
    pub previous: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
    /// Signed Java ItemEntity age held as two's-complement u32 for existing
    /// callers. -32768 never ages; -6000 is extended life.
    pub age: u32,
    /// Entity.tickCount; independent of item age (which may be loaded/merged).
    pub tick_count: u32,
    pub pickup_delay: u32,
    pub bob_offset: f32,
}
/// Environment results needed by the entity owner, including lethal lava damage.
#[derive(Debug)]
pub struct ItemEnvironmentTick {
    pub alive: bool,
    pub lava_hurt: bool,
}
impl ItemEnvironmentTick {
    pub fn should_play_lava_hurt_sound(&self, tick: u32) -> bool {
        self.lava_hurt && (!self.alive || tick.is_multiple_of(10))
    }
}
impl ItemEntity {
    pub const WIDTH: f64 = 0.25;
    /// Java ItemEntity.setUnlimitedLifetime: age -32768 is never incremented
    /// and is excluded from nearby stack merging.
    pub const UNLIMITED_AGE: u32 = (-32768_i32) as u32;
    pub fn bounds(&self) -> Aabb {
        let h = Self::WIDTH / 2.0;
        Aabb::new(
            Vec3::new(self.position.x - h, self.position.y, self.position.z - h),
            Vec3::new(
                self.position.x + h,
                self.position.y + Self::WIDTH,
                self.position.z + h,
            ),
        )
    }
    pub fn environment_tick(
        &mut self,
        world: &World,
        facts: &BlockFacts,
        registry: &horizon_world::registry::BlockRegistry,
        items: &crate::items::Items,
    ) -> ItemEnvironmentTick {
        let at = world.block(
            self.position.x.floor() as i32,
            (self.position.y + 0.1).floor() as i32,
            self.position.z.floor() as i32,
        );
        let fluid = facts.get(at).fluid;
        let fire_immune = items.resists_fire(&self.stack, "minecraft:in_fire");
        if fluid == Fluid::Water || fire_immune {
            self.fire_ticks = 0;
        }
        if self.fire_ticks > 0 {
            if self.fire_ticks % 20 == 0
                && fluid != Fluid::Lava
                && !items.resists_fire(&self.stack, "minecraft:on_fire")
            {
                self.health -= 1;
            }
            self.fire_ticks -= 1;
        }
        let mut lava_hurt = false;
        if fluid == Fluid::Lava && !fire_immune {
            self.fire_ticks = self.fire_ticks.max(300);
            if !items.resists_fire(&self.stack, "minecraft:lava") {
                self.health -= 4;
                lava_hurt = true;
            }
        }
        let fire_damage = crate::motion::environment::fire_contact(world, registry, self.bounds());
        if fire_damage > 0.0 && !fire_immune {
            self.fire_ticks = self.fire_ticks.max(160);
            if !items.resists_fire(&self.stack, "minecraft:in_fire") {
                self.health -= fire_damage as i32;
            }
        }
        ItemEnvironmentTick {
            alive: self.health > 0,
            lava_hurt,
        }
    }
    /// Returns false when the item should be removed.
    pub fn tick(
        &mut self,
        world: &World,
        facts: &BlockFacts,
        collision: &BlockCollisionRegistry<'_>,
    ) -> bool {
        self.tick_count = self.tick_count.wrapping_add(1);
        self.previous = self.position;
        if self.pickup_delay > 0 && self.pickup_delay != u32::MAX {
            self.pickup_delay -= 1;
        }
        let eye = world.block(
            self.position.x.floor() as i32,
            (self.position.y + 0.1).floor() as i32,
            self.position.z.floor() as i32,
        );
        let in_water = facts.get(eye).fluid == Fluid::Water;
        if in_water {
            // ItemEntity.setUnderwaterMovement
            self.velocity = Vec3::new(
                self.velocity.x * 0.99,
                self.velocity.y + if self.velocity.y < 0.06 { 5.0e-4 } else { 0.0 },
                self.velocity.z * 0.99,
            );
        } else if facts.get(eye).fluid == Fluid::Lava {
            self.velocity = Vec3::new(
                self.velocity.x * 0.95,
                self.velocity.y + if self.velocity.y < 0.06 { 5.0e-4 } else { 0.0 },
                self.velocity.z * 0.95,
            );
        } else {
            self.velocity.y -= 0.04;
        }
        let horizontal = self.velocity.x * self.velocity.x + self.velocity.z * self.velocity.z;
        if !self.on_ground
            || horizontal > 1.0e-5
            || self.age.wrapping_add(self.id).is_multiple_of(4)
        {
            let mut scene = SceneCollisions {
                world,
                binding: collision,
                subject: None,
            };
            let wanted = self.velocity;
            let moved = scene
                .resolve(self.bounds(), wanted, 0.0, self.on_ground)
                .unwrap_or(Vec3::ZERO);
            self.position = Vec3::new(
                self.position.x + moved.x,
                self.position.y + moved.y,
                self.position.z + moved.z,
            );
            self.on_ground = wanted.y < 0.0 && moved.y != wanted.y;
            if moved.x != wanted.x {
                self.velocity.x = 0.0;
            }
            if moved.z != wanted.z {
                self.velocity.z = 0.0;
            }
            if moved.y != wanted.y {
                self.velocity.y = 0.0;
            }
            let mut friction = 0.98;
            if self.on_ground {
                let below = world.block(
                    self.position.x.floor() as i32,
                    (self.position.y - 0.999_999).floor() as i32,
                    self.position.z.floor() as i32,
                );
                friction = f64::from(facts.get(below).friction) * 0.98;
            }
            self.velocity = Vec3::new(
                self.velocity.x * friction,
                self.velocity.y * 0.98,
                self.velocity.z * friction,
            );
            if self.on_ground && self.velocity.y < 0.0 {
                self.velocity.y *= -0.5;
            }
        }
        if self.age != Self::UNLIMITED_AGE {
            self.age = self.age.wrapping_add(1);
        }
        (self.age as i32) < 6000 && self.position.y > -128.0
    }
    /// `ItemEntity.isMergable` at `age`: a stack below its size whose pickup
    /// delay isn't infinite, before its lifetime ends.
    fn mergeable(&self, age: u32, max_stack: u32) -> bool {
        self.pickup_delay != u32::MAX && age < 6000 && self.stack.count < max_stack
    }
}
/// Dropped items by block cell, for `ItemEntity.mergeWithNeighbours`'s
/// search around an item. Holds indexes into the tick's item list.
#[derive(Default)]
pub struct ItemCells {
    cells: crate::fast_hash::FxHashMap<(i32, i32, i32), Vec<u32>>,
}
impl ItemCells {
    pub fn new(items: &[ItemEntity]) -> Self {
        let mut cells = Self::default();
        for (i, item) in items.iter().enumerate() {
            cells.add(i, item.position);
        }
        cells
    }
    fn cell(p: Vec3) -> (i32, i32, i32) {
        (p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32)
    }
    fn add(&mut self, i: usize, at: Vec3) {
        self.cells.entry(Self::cell(at)).or_default().push(i as u32);
    }
    pub fn remove(&mut self, i: usize, at: Vec3) {
        if let Some(list) = self.cells.get_mut(&Self::cell(at)) {
            list.retain(|&j| j as usize != i);
        }
    }
    pub fn moved(&mut self, i: usize, from: Vec3, to: Vec3) {
        if Self::cell(from) != Self::cell(to) {
            self.remove(i, from);
            self.add(i, to);
        }
    }
    /// The items whose cells could hold a box meeting `area`, in the order
    /// they were added (vanilla lists a section's entities in the order they
    /// entered it).
    fn near(&self, area: Aabb, out: &mut Vec<u32>) {
        out.clear();
        // An item's box reaches 0.125 sideways and 0.25 up from its position.
        let (min, max) = (area.min_position(), area.max_position());
        let (lo, hi) = (
            Self::cell(Vec3::new(min.x - 0.125, min.y - 0.25, min.z - 0.125)),
            Self::cell(Vec3::new(max.x + 0.125, max.y, max.z + 0.125)),
        );
        for x in lo.0..=hi.0 {
            for y in lo.1..=hi.1 {
                for z in lo.2..=hi.2 {
                    if let Some(list) = self.cells.get(&(x, y, z)) {
                        out.extend_from_slice(list);
                    }
                }
            }
        }
        out.sort_unstable();
    }
}
/// `ItemEntity.mergeWithNeighbours` for `items[i]`, which has just moved in
/// its tick (its `age` already counts this tick; vanilla's doesn't yet): each
/// mergeable item whose box meets this one's grown by 0.5 sideways takes this
/// stack or gives it its own, the smaller into the larger
/// (`ItemEntity.tryToMerge`), until this item is used up. An emptied item is
/// marked in `removed`.
pub fn merge_with_neighbours(
    items: &mut [ItemEntity],
    removed: &mut [bool],
    cells: &mut ItemCells,
    i: usize,
    max_stack: &dyn Fn(&ItemStack) -> u32,
) {
    let age = |items: &[ItemEntity], j: usize| {
        if j == i {
            items[j].age - 1
        } else {
            items[j].age
        }
    };
    if removed[i] || !items[i].mergeable(age(items, i), max_stack(&items[i].stack)) {
        return;
    }
    let area = items[i].bounds().inflate(Vec3::new(0.5, 0.0, 0.5));
    let mut near = Vec::new();
    cells.near(area, &mut near);
    for j in near {
        let j = j as usize;
        if j == i
            || removed[j]
            || !items[j].bounds().intersects(area)
            || !items[j].mergeable(age(items, j), max_stack(&items[j].stack))
        {
            continue;
        }
        // ItemEntity.areMergable: both fit one stack of the same item and components.
        let (this, other) = (&items[i].stack, &items[j].stack);
        if this.count + other.count > max_stack(other) || !this.same_item_same_components(other) {
            continue;
        }
        let (to, from) = if other.count < this.count {
            (i, j)
        } else {
            (j, i)
        };
        // ItemEntity.merge(toStack, fromStack, 64).
        let room = max_stack(&items[to].stack).min(64) - items[to].stack.count;
        let moved = room.min(items[from].stack.count);
        items[to].stack.count += moved;
        items[from].stack.count -= moved;
        let (delay, from_age, to_age) =
            (items[from].pickup_delay, age(items, from), age(items, to));
        let to_item = &mut items[to];
        to_item.pickup_delay = to_item.pickup_delay.max(delay);
        // This item's age goes up once its tick ends.
        to_item.age = to_age.min(from_age) + u32::from(to == i);
        if items[from].stack.count == 0 {
            removed[from] = true;
            cells.remove(from, items[from].position);
        }
        if removed[i] {
            return;
        }
    }
}
/// Java 26.3 `ItemEntity.mergeWithNeighbours` following each entity tick.
/// The player-facing entity tracker observes the *survivor's* changed stack
/// and the disappeared donor; this function does not emit network packets.
/// `max_stack` reads the registry's max stack size (usually 64).
pub fn merge_nearby_items(
    items: &mut Vec<ItemEntity>,
    max_stack: impl Fn(&str) -> u32,
    actively_ticking: impl Fn(&ItemEntity) -> bool,
) {
    if items.len() < 2 {
        return;
    }
    // Stationary items only scan on tick 40, 80, etc. Return before making
    // a full spatial index when *no active, mergeable item* is due to scan.
    // No earlier item may alter the candidate state before this first scan.
    let Some(first_due) = items.iter().position(|item| {
        actively_ticking(item) && item_can_merge(item, &max_stack) && item_merge_scan_due(item)
    }) else {
        return;
    };
    let mut removed = vec![false; items.len()];
    // A single per-tick spatial grid turns distant pairs into constant-sized
    // neighbourhood lookups. Sorted candidate indexes preserve the exact
    // original entity iteration order (important for equal-stack winners).
    let mut cells = ItemCells::new(items);
    // In piles occupying a single cell, the index repeatedly sorts nearly
    // all the items and removing donors shifts the cell's Vec. On measured
    // 64/256/1024-item dense workloads that was slower than the old scan.
    // Keep the exact original all-pairs order for high-occupancy cells.
    const MAX_INDEXED_CELL: usize = 24;
    let indexed = cells
        .cells
        .values()
        .all(|cell| cell.len() <= MAX_INDEXED_CELL);
    let mut nearby = Vec::new();
    for i in first_due..items.len() {
        // The first initiator was just validated. Check later initiators at
        // their actual iteration time, since earlier merges may change them.
        if i != first_due
            && (removed[i]
                || !actively_ticking(&items[i])
                || !item_can_merge(&items[i], &max_stack)
                || !item_merge_scan_due(&items[i]))
        {
            continue;
        }
        if indexed {
            let area = items[i].bounds().inflate(Vec3::new(0.5, 0.0, 0.5));
            cells.near(area, &mut nearby);
        } else {
            nearby.clear();
            nearby.extend(0..items.len() as u32);
        }
        for &j in &nearby {
            let j = j as usize;
            if i == j || removed[i] || removed[j] || !item_can_merge(&items[j], &max_stack) {
                continue;
            }
            // getEntitiesOfClass in bounding box inflated by (.5, 0, .5).
            // Two item AABBs are .25 across; exact overlap uses strict <.
            let (a, b) = (&items[i], &items[j]);
            if (a.position.x - b.position.x).abs() >= 0.75
                || (a.position.y - b.position.y).abs() >= ItemEntity::WIDTH
                || (a.position.z - b.position.z).abs() >= 0.75
                || a.target != b.target
                || !a.stack.same_item_same_components(&b.stack)
            {
                continue;
            }
            // tryToMerge: the larger stack always survives; when equal,
            // the *other* entity survives. Capacity is min(64, maxStack).
            let (receiver, donor) = if a.stack.count > b.stack.count {
                (i, j)
            } else {
                (j, i)
            };
            let (receiver_item, donor_item) = if receiver < donor {
                let (head, tail) = items.split_at_mut(donor);
                (&mut head[receiver], &mut tail[0])
            } else {
                let (head, tail) = items.split_at_mut(receiver);
                (&mut tail[0], &mut head[donor])
            };
            let cap = max_stack(&receiver_item.stack.item).min(64);
            let moved = donor_item
                .stack
                .count
                .min(cap.saturating_sub(receiver_item.stack.count));
            if moved == 0 {
                continue;
            }
            receiver_item.stack.count += moved;
            donor_item.stack.count -= moved;
            receiver_item.pickup_delay = receiver_item.pickup_delay.max(donor_item.pickup_delay);
            // Java stores Age as signed int: extended (-6000) must win
            // over an ordinary positive age when two items merge.
            receiver_item.age = (receiver_item.age as i32).min(donor_item.age as i32) as u32;
            if donor_item.stack.count == 0 {
                removed[donor] = true;
                // Already removed donors never participate in subsequent
                // neighbourhood queries. Avoid dirtying the original order.
                if indexed {
                    cells.remove(donor, donor_item.position);
                }
            }
        }
    }
    let mut next = 0;
    items.retain(|_| {
        let keep = !removed[next];
        next += 1;
        keep
    });
}

/// Original O(n²) implementation, frozen as an independent equivalence oracle
/// for regression and benchmarking of the spatial-index path.
#[cfg(test)]
fn merge_nearby_items_scan_reference(
    items: &mut Vec<ItemEntity>,
    max_stack: impl Fn(&str) -> u32,
    actively_ticking: impl Fn(&ItemEntity) -> bool,
) {
    if items.len() < 2 {
        return;
    }
    let mut removed = vec![false; items.len()];
    for i in 0..items.len() {
        if removed[i] || !actively_ticking(&items[i]) || !item_can_merge(&items[i], &max_stack) {
            continue;
        }
        // ItemEntity.tick: crossing an integer block coordinate checks every
        // 2 ticks, otherwise every 40; the entity tickCount, not its Age.
        let a = &items[i];
        let crossed = (a.previous.x.floor() != a.position.x.floor())
            || (a.previous.y.floor() != a.position.y.floor())
            || (a.previous.z.floor() != a.position.z.floor());
        let period = if crossed { 2 } else { 40 };
        if !a.tick_count.is_multiple_of(period) {
            continue;
        }
        for j in 0..items.len() {
            if i == j || removed[i] || removed[j] || !item_can_merge(&items[j], &max_stack) {
                continue;
            }
            // getEntitiesOfClass in bounding box inflated by (.5, 0, .5).
            // Two item AABBs are .25 across; exact overlap uses strict <.
            let (a, b) = (&items[i], &items[j]);
            if (a.position.x - b.position.x).abs() >= 0.75
                || (a.position.y - b.position.y).abs() >= ItemEntity::WIDTH
                || (a.position.z - b.position.z).abs() >= 0.75
                || a.target != b.target
                || !a.stack.same_item_same_components(&b.stack)
            {
                continue;
            }
            // tryToMerge: the larger stack always survives; when equal,
            // the *other* entity survives. Capacity is min(64, maxStack).
            let (receiver, donor) = if a.stack.count > b.stack.count {
                (i, j)
            } else {
                (j, i)
            };
            let (receiver_item, donor_item) = if receiver < donor {
                let (head, tail) = items.split_at_mut(donor);
                (&mut head[receiver], &mut tail[0])
            } else {
                let (head, tail) = items.split_at_mut(receiver);
                (&mut tail[0], &mut head[donor])
            };
            let cap = max_stack(&receiver_item.stack.item).min(64);
            let moved = donor_item
                .stack
                .count
                .min(cap.saturating_sub(receiver_item.stack.count));
            if moved == 0 {
                continue;
            }
            receiver_item.stack.count += moved;
            donor_item.stack.count -= moved;
            receiver_item.pickup_delay = receiver_item.pickup_delay.max(donor_item.pickup_delay);
            // Java stores Age as signed int: extended (-6000) must win
            // over an ordinary positive age when two items merge.
            receiver_item.age = (receiver_item.age as i32).min(donor_item.age as i32) as u32;
            if donor_item.stack.count == 0 {
                removed[donor] = true;
            }
        }
    }
    let mut next = 0;
    items.retain(|_| {
        let keep = !removed[next];
        next += 1;
        keep
    });
}

/// ItemEntity's own check cadence, independent of saved Age.
fn item_merge_scan_due(a: &ItemEntity) -> bool {
    let crossed = (a.previous.x.floor() != a.position.x.floor())
        || (a.previous.y.floor() != a.position.y.floor())
        || (a.previous.z.floor() != a.position.z.floor());
    a.tick_count.is_multiple_of(if crossed { 2 } else { 40 })
}

fn item_can_merge(item: &ItemEntity, max_stack: &impl Fn(&str) -> u32) -> bool {
    item.health > 0
        && item.pickup_delay != 32767
        && item.pickup_delay != u32::MAX
        && item.age != ItemEntity::UNLIMITED_AGE
        && (item.age as i32) < 6000
        && item.stack.count > 0
        && item.stack.count < max_stack(&item.stack.item).min(64)
}

/// ExperienceOrb: gravity 0.03, homing towards the player within 8 blocks.
#[derive(Clone, Debug)]
pub struct ExperienceOrb {
    /// The entity id servers track it by.
    pub id: u32,
    pub position: Vec3,
    pub previous: Vec3,
    pub velocity: Vec3,
    pub value: u32,
    pub age: u32,
    pub on_ground: bool,
}
impl ExperienceOrb {
    /// ExperienceOrb.getExperienceValue: the largest orb size not above `value`.
    pub fn split(value: u32) -> u32 {
        [2477, 1237, 617, 307, 149, 73, 37, 17, 7, 3, 1]
            .into_iter()
            .find(|&v| value >= v)
            .unwrap_or(1)
    }
    /// ExperienceOrb.getIcon
    pub fn icon(&self) -> u32 {
        match self.value {
            2477.. => 10,
            1237.. => 9,
            617.. => 8,
            307.. => 7,
            149.. => 6,
            73.. => 5,
            37.. => 4,
            17.. => 3,
            7.. => 2,
            3.. => 1,
            _ => 0,
        }
    }
    pub fn tick(
        &mut self,
        world: &World,
        facts: &BlockFacts,
        collision: &BlockCollisionRegistry<'_>,
        player: Option<[f64; 3]>,
    ) -> bool {
        self.previous = self.position;
        let block = world.block(
            self.position.x.floor() as i32,
            self.position.y.floor() as i32,
            self.position.z.floor() as i32,
        );
        if facts.get(block).fluid == Fluid::Water {
            self.velocity = Vec3::new(
                self.velocity.x * 0.99,
                (self.velocity.y + 5.0e-4).min(0.06),
                self.velocity.z * 0.99,
            );
        } else {
            self.velocity.y -= 0.03;
        }
        if let Some(p) = player {
            let d = [
                (p[0] - self.position.x) / 8.0,
                (p[1] - self.position.y) / 8.0,
                (p[2] - self.position.z) / 8.0,
            ];
            let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let power = 1.0 - length;
            if power > 0.0 && length > 1e-6 {
                let power = power * power;
                self.velocity = Vec3::new(
                    self.velocity.x + d[0] / length * power * 0.1,
                    self.velocity.y + d[1] / length * power * 0.1,
                    self.velocity.z + d[2] / length * power * 0.1,
                );
            }
        }
        let h = 0.25;
        let bounds = Aabb::new(
            Vec3::new(self.position.x - h, self.position.y, self.position.z - h),
            Vec3::new(
                self.position.x + h,
                self.position.y + 0.5,
                self.position.z + h,
            ),
        );
        let mut scene = SceneCollisions {
            world,
            binding: collision,
            subject: None,
        };
        let wanted = self.velocity;
        let moved = scene
            .resolve(bounds, wanted, 0.0, self.on_ground)
            .unwrap_or(Vec3::ZERO);
        self.position = Vec3::new(
            self.position.x + moved.x,
            self.position.y + moved.y,
            self.position.z + moved.z,
        );
        self.on_ground = wanted.y < 0.0 && moved.y != wanted.y;
        let mut friction = 0.98;
        if self.on_ground {
            let below = world.block(
                self.position.x.floor() as i32,
                (self.position.y - 1.0).floor() as i32,
                self.position.z.floor() as i32,
            );
            friction = f64::from(facts.get(below).friction) * 0.98;
        }
        self.velocity = Vec3::new(
            self.velocity.x * friction,
            self.velocity.y * 0.98,
            self.velocity.z * friction,
        );
        if self.on_ground {
            self.velocity.y *= -0.9;
        }
        self.age += 1;
        self.age < 6000
    }
}
/// Player experience (Player.giveExperiencePoints / getXpNeededForNextLevel).
#[derive(Clone, Copy, Debug, Default)]
pub struct Experience {
    pub level: i32,
    pub progress: f32,
    pub total: i32,
}
impl Experience {
    /// `Player.getBaseExperienceReward`: keep-inventory and spectators drop none.
    pub fn death_reward(&self, keep: bool) -> u32 {
        if keep {
            0
        } else {
            self.level.wrapping_mul(7).clamp(0, 100) as u32
        }
    }
    /// `ServerPlayer.restoreFrom`: only keep-inventory and spectators retain XP.
    pub fn respawn(&mut self, keep: bool) {
        if !keep {
            *self = Self::default();
        }
    }
    pub fn needed(level: i32) -> i32 {
        if level >= 30 {
            112 + (level - 30) * 9
        } else if level >= 15 {
            37 + (level - 15) * 5
        } else {
            7 + level * 2
        }
    }
    /// Returns true when a level was gained.
    pub fn give(&mut self, points: i32) -> bool {
        let start = self.level;
        self.progress += points as f32 / Self::needed(self.level) as f32;
        self.total = self.total.saturating_add(points).max(0);
        while self.progress >= 1.0 {
            self.progress = (self.progress - 1.0) * Self::needed(self.level) as f32;
            self.level += 1;
            self.progress /= Self::needed(self.level) as f32;
        }
        self.level > start
    }
}
/// A painting (`Painting`, a `HangingEntity`): the block it hangs in, the
/// 2D data value of the direction it faces, and its variant.
#[derive(Clone, Debug)]
pub struct Painting {
    pub id: u32,
    pub pos: [i32; 3],
    /// Direction.get2DDataValue: south 0, west 1, north 2, east 3.
    pub facing: u8,
    pub variant: std::sync::Arc<str>,
    pub width: i32,
    pub height: i32,
    /// BlockAttachedEntity.checkInterval.
    pub check_interval: u32,
}
impl Painting {
    pub fn step(&self) -> [i32; 3] {
        [[0, 0, 1], [-1, 0, 0], [0, 0, -1], [1, 0, 0]][usize::from(self.facing & 3)]
    }
    /// Direction.getCounterClockWise (the painting's left as it faces out).
    pub fn left(&self) -> [i32; 3] {
        [[1, 0, 0], [0, 0, 1], [-1, 0, 0], [0, 0, -1]][usize::from(self.facing & 3)]
    }
    /// Painting.calculateBoundingBox: (min, max).
    pub fn bounds(&self) -> ([f64; 3], [f64; 3]) {
        let s = self.step().map(f64::from);
        let l = self.left().map(f64::from);
        let off = |n: i32| if n % 2 == 0 { 0.5 } else { 0.0 };
        let (h, v) = (off(self.width), off(self.height));
        let c = [
            f64::from(self.pos[0]) + 0.5 - s[0] * 0.46875 + l[0] * h,
            f64::from(self.pos[1]) + 0.5 + v,
            f64::from(self.pos[2]) + 0.5 - s[2] * 0.46875 + l[2] * h,
        ];
        let along_x = s[0] != 0.0;
        let size = [
            if along_x {
                0.0625
            } else {
                f64::from(self.width)
            },
            f64::from(self.height),
            if along_x {
                f64::from(self.width)
            } else {
                0.0625
            },
        ];
        (
            std::array::from_fn(|i| c[i] - size[i] / 2.0),
            std::array::from_fn(|i| c[i] + size[i] / 2.0),
        )
    }
}
/// An item frame or glow item frame (`ItemFrame`).
#[derive(Clone, Debug)]
pub struct ItemFrame {
    pub id: u32,
    pub pos: [i32; 3],
    /// Direction.get3DDataValue: down 0, up 1, north 2, south 3, west 4, east 5.
    pub facing: u8,
    pub glow: bool,
    pub item: Option<crate::items::ItemStack>,
    /// 0..8, in 45 degree steps.
    pub rotation: u8,
    pub drop_chance: f32,
    pub fixed: bool,
    pub invisible: bool,
    pub check_interval: u32,
}
impl ItemFrame {
    pub fn step(&self) -> [i32; 3] {
        [
            [0, -1, 0],
            [0, 1, 0],
            [0, 0, -1],
            [0, 0, 1],
            [-1, 0, 0],
            [1, 0, 0],
        ][usize::from(self.facing.min(5))]
    }
    pub fn kind(&self) -> &'static str {
        if self.glow {
            "minecraft:glow_item_frame"
        } else {
            "minecraft:item_frame"
        }
    }
    /// ItemFrame.createBoundingBox (without a framed map): (min, max).
    pub fn bounds(&self) -> ([f64; 3], [f64; 3]) {
        let s = self.step().map(f64::from);
        let c: [f64; 3] = std::array::from_fn(|i| f64::from(self.pos[i]) + 0.5 - s[i] * 0.46875);
        let size: [f64; 3] = std::array::from_fn(|i| if s[i] != 0.0 { 0.0625 } else { 0.75 });
        (
            std::array::from_fn(|i| c[i] - size[i] / 2.0),
            std::array::from_fn(|i| c[i] + size[i] / 2.0),
        )
    }
}
/// An armor stand (`ArmorStand`).
#[derive(Clone, Debug)]
pub struct ArmorStand {
    pub fire_ticks: i32,
    pub health: f32,
    pub id: u32,
    pub position: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub yaw: f32,
    /// mainhand, offhand, feet, legs, chest, head (EquipmentSlot order).
    pub equipment: [Option<crate::items::ItemStack>; 6],
    pub small: bool,
    pub show_arms: bool,
    pub no_base_plate: bool,
    pub invisible: bool,
    pub marker: bool,
    /// Game time of the last hit (the wobble and the second-hit window).
    pub last_hit: i64,
}
impl ArmorStand {
    pub const SLOTS: [&'static str; 6] = ["mainhand", "offhand", "feet", "legs", "chest", "head"];
    pub fn new(id: u32, position: Vec3, yaw: f32) -> Self {
        Self {
            fire_ticks: 0,
            health: 20.0,
            id,
            position,
            velocity: Vec3::ZERO,
            on_ground: true,
            yaw,
            equipment: Default::default(),
            small: false,
            show_arms: false,
            no_base_plate: false,
            invisible: false,
            marker: false,
            last_hit: i64::MIN / 2,
        }
    }
    pub fn environment_tick(
        &mut self,
        world: &World,
        facts: &BlockFacts,
        registry: &horizon_world::registry::BlockRegistry,
    ) -> bool {
        let (lo, hi) = (self.bounds().min_position(), self.bounds().max_position());
        let mut water = false;
        let mut lava = false;
        for x in lo.x.floor() as i32..=hi.x.floor() as i32 {
            for y in lo.y.floor() as i32..hi.y.ceil() as i32 {
                for z in lo.z.floor() as i32..=hi.z.floor() as i32 {
                    match facts.get(world.block(x, y, z)).fluid {
                        Fluid::Water => water = true,
                        Fluid::Lava => lava = true,
                        _ => {}
                    }
                }
            }
        }
        if water {
            self.fire_ticks = 0;
        }
        if self.fire_ticks > 0 {
            if self.fire_ticks % 20 == 0
                && !lava
                && !self.invisible
                && !self.marker
                && self.health > 0.5
            {
                self.health -= 4.0;
            }
            self.fire_ticks -= 1;
        }
        if lava {
            self.fire_ticks = self.fire_ticks.max(300);
        }
        let fire = crate::motion::environment::fire_contact(world, registry, self.bounds()) > 0.0;
        if fire && !self.invisible && !self.marker {
            if self.fire_ticks > 0 {
                self.health -= 0.15;
            } else {
                self.fire_ticks = 100;
            }
        }
        self.health > 0.5
    }
    /// EntityType.ARMOR_STAND dimensions (0.5 x 1.975, half for small).
    pub fn bounds(&self) -> Aabb {
        let s = if self.small { 0.5 } else { 1.0 };
        let (w, h) = (0.25 * s, 1.975 * s);
        let p = self.position;
        Aabb::new(
            Vec3::new(p.x - w, p.y, p.z - w),
            Vec3::new(p.x + w, p.y + h, p.z + w),
        )
    }
    /// LivingEntity.travelInAir (armor stands have no other movement): move
    /// by the velocity (a landing stops the fall,
    /// `Block.updateEntityMovementAfterFallOn`), then gravity 0.08 and the
    /// 0.98 drag, so a stand at rest on the ground keeps -0.0784 as vanilla's.
    pub fn tick(&mut self, scene: &mut crate::collisions::SceneCollisions) {
        if self.marker {
            return;
        }
        let wanted = self.velocity;
        let mut y = wanted.y;
        if let Ok(moved) = scene.resolve(self.bounds(), wanted, 0.0, self.on_ground) {
            self.position = Vec3::new(
                self.position.x + moved.x,
                self.position.y + moved.y,
                self.position.z + moved.z,
            );
            self.on_ground = wanted.y < 0.0 && moved.y > wanted.y;
            if moved.y != wanted.y {
                y = 0.0;
            }
        }
        self.velocity = Vec3::new(0.0, (y - 0.08) * f64::from(0.98f32), 0.0);
    }
}

#[cfg(test)]
mod experience_tests {
    use super::*;

    #[test]
    fn death_reward_uses_levels_and_caps_the_recoverable_points() {
        for (level, reward) in [(0, 0), (1, 7), (14, 98), (15, 100), (30, 100)] {
            let xp = Experience {
                level,
                progress: 0.75,
                total: 10_000,
            };
            assert_eq!(xp.death_reward(false), reward);
            assert_eq!(xp.death_reward(true), 0);
        }
    }

    #[test]
    fn respawn_clears_all_xp_unless_it_is_kept() {
        let mut xp = Experience::default();
        xp.give(155);
        let before = (xp.level, xp.progress, xp.total);
        xp.respawn(true);
        assert_eq!((xp.level, xp.progress, xp.total), before);
        xp.respawn(false);
        assert_eq!((xp.level, xp.progress, xp.total), (0, 0.0, 0));
        xp.give(7);
        assert_eq!((xp.level, xp.progress, xp.total), (1, 0.0, 7));
    }
}

#[cfg(test)]
mod fire_environment_tests {
    use super::*;
    #[test]
    fn ordinary_items_burn_but_native_netherite_resists_lava() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.local/horizon-client")
            .join(horizon_core::VANILLA_VERSION);
        if !root.join("items.json").exists() {
            return;
        }
        let assets = crate::assets::Assets::load(&root).unwrap();
        let render = assets.reload(&root).unwrap();
        let facts = BlockFacts::load(&root, assets.registry).unwrap();
        let items = crate::items::Items::load(&root, assets.registry, &facts).unwrap();
        let mut world = World::test_scene(&assets, &render, 3).unwrap();
        let at = Vec3::new(20.5, 4.0, 20.5);
        world
            .set(
                20,
                4,
                20,
                assets.registry.default_state("minecraft:lava").unwrap(),
            )
            .unwrap();
        let make = |id: &str| ItemEntity {
            id: 1,
            thrower: None,
            target: None,
            stack: ItemStack {
                item: std::sync::Arc::from(id),
                count: 1,
                damage: 0,
                extra: None,
            },
            health: 5,
            fire_ticks: 0,
            position: at,
            previous: at,
            velocity: Vec3::ZERO,
            on_ground: false,
            age: 0,
            tick_count: 0,
            pickup_delay: 0,
            bob_offset: 0.0,
        };
        let mut wood = make("minecraft:oak_planks");
        let mut metal = make("minecraft:netherite_ingot");
        let first = wood.environment_tick(&world, &facts, assets.registry, &items);
        assert!(first.alive && first.lava_hurt);
        assert!(
            !first.should_play_lava_hurt_sound(1),
            "ordinary entry damage must not repeat the burn sound"
        );
        assert!(first.should_play_lava_hurt_sound(10));
        assert!(
            wood.fire_ticks > 0,
            "lava contact must ignite dropped items"
        );
        let last = wood.environment_tick(&world, &facts, assets.registry, &items);
        assert!(
            !last.alive && last.lava_hurt,
            "lethal lava damage still emits burn sound"
        );
        assert!(
            last.should_play_lava_hurt_sound(2),
            "the lethal hit must always play the sound"
        );
        for _ in 0..40 {
            let result = metal.environment_tick(&world, &facts, assets.registry, &items);
            assert!(result.alive && !result.lava_hurt);
            assert!(!result.should_play_lava_hurt_sound(10));
        }
        assert_eq!(metal.health, 5);
        assert_eq!(metal.fire_ticks, 0, "fire-resistant items must not ignite");
        world
            .set(
                20,
                4,
                20,
                assets.registry.default_state("minecraft:fire").unwrap(),
            )
            .unwrap();
        let mut burning = make("minecraft:oak_planks");
        let result = burning.environment_tick(&world, &facts, assets.registry, &items);
        assert!(result.alive && !result.lava_hurt);
        assert!(burning.fire_ticks > 0);
        world
            .set(
                20,
                4,
                20,
                assets
                    .registry
                    .default_state("minecraft:soul_fire")
                    .unwrap(),
            )
            .unwrap();
        let mut soul = make("minecraft:oak_planks");
        soul.environment_tick(&world, &facts, assets.registry, &items);
        assert_eq!(soul.health, 3);
        let mut stand = ArmorStand::new(2, at, 0.0);
        world
            .set(
                20,
                4,
                20,
                assets.registry.default_state("minecraft:lava").unwrap(),
            )
            .unwrap();
        for _ in 0..150 {
            assert!(stand.environment_tick(&world, &facts, assets.registry));
        }
        assert_eq!(stand.health, 20.0, "lava does not hurt armor stands");
    }
}

#[cfg(test)]
mod item_merge_parity_tests {
    use super::*;

    fn item(id: u32, item: &str, count: u32) -> ItemEntity {
        ItemEntity {
            id,
            stack: ItemStack::new(item, count),
            target: None,
            thrower: None,
            health: 5,
            fire_ticks: 0,
            position: Vec3::new(0.5, 65.0, 0.5),
            previous: Vec3::new(0.5, 65.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            age: 10,
            tick_count: 0,
            pickup_delay: 20,
            bob_offset: 0.0,
        }
    }

    fn run(items: &mut Vec<ItemEntity>) {
        merge_nearby_items(
            items,
            |id| {
                if id == "minecraft:diamond_sword" {
                    1
                } else if id == "minecraft:snowball" {
                    16
                } else {
                    64
                }
            },
            |_| true,
        );
    }

    #[test]
    fn stationary_check_uses_entity_tick_count_not_saved_age() {
        let mut left = item(1, "minecraft:stick", 1);
        left.age = 135;
        left.tick_count = 39;
        let mut right = item(2, "minecraft:stick", 8);
        right.tick_count = 39;
        let mut items = vec![left, right];
        run(&mut items);
        assert_eq!(items.len(), 2, "no merge before stationary tick 40");
        for item in &mut items {
            item.tick_count = 40;
        }
        run(&mut items);
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].id, items[0].stack.count), (2, 9));
        assert_eq!(items[0].age, 10, "survivor adopts donor's earlier age");
    }

    #[test]
    fn boundary_crossing_checks_every_second_tick() {
        let mut a = item(1, "minecraft:stick", 1);
        let mut b = item(2, "minecraft:stick", 1);
        a.previous.x = -0.2;
        a.tick_count = 1;
        b.tick_count = 1;
        let mut items = vec![a, b];
        run(&mut items);
        assert_eq!(items.len(), 2);
        items[0].tick_count = 2;
        run(&mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, 2, "equal counts merge into other entity");
        assert_eq!(items[0].stack.count, 2);
    }

    #[test]
    fn owner_target_not_thrower_controls_merge_compatibility() {
        let mut a = item(1, "minecraft:stick", 1);
        let mut b = item(2, "minecraft:stick", 8);
        a.tick_count = 40;
        a.thrower = Some([1, 0, 0, 0]);
        b.thrower = Some([2, 0, 0, 0]);
        a.target = Some([3, 0, 0, 0]);
        b.target = Some([4, 0, 0, 0]);
        let mut items = vec![a, b];
        run(&mut items);
        assert_eq!(items.len(), 2, "different Owner tags must not merge");
        items[1].target = items[0].target;
        run(&mut items);
        assert_eq!(items.len(), 1, "different Thrower tags may merge");
    }

    #[test]
    fn item_components_and_item_type_prevent_merging() {
        let mut a = item(1, "minecraft:stick", 1);
        a.tick_count = 40;
        let mut b = item(2, "minecraft:stone", 8);
        let mut items = vec![a.clone(), b.clone()];
        run(&mut items);
        assert_eq!(items.len(), 2);
        b.stack = ItemStack::new("minecraft:stick", 8);
        b.stack.damage = 1;
        items = vec![a.clone(), b.clone()];
        run(&mut items);
        assert_eq!(items.len(), 2, "damage components must match");
        b.stack.damage = 0;
        b.stack.data_mut().repair_cost = 2;
        items = vec![a, b];
        run(&mut items);
        assert_eq!(items.len(), 2, "extra components must match");
    }

    #[test]
    fn precise_aabb_overlap_and_special_delay_must_allow_merge() {
        let mut a = item(1, "minecraft:stick", 1);
        a.tick_count = 40;
        let mut b = item(2, "minecraft:stick", 8);
        b.position.x = 1.25;
        let mut items = vec![a.clone(), b.clone()];
        run(&mut items);
        assert_eq!(items.len(), 2, "x boundary is exclusive");
        b.position.x = 1.2;
        b.position.y += 0.25;
        items = vec![a.clone(), b.clone()];
        run(&mut items);
        assert_eq!(items.len(), 2, "y boundary is exclusive");
        b.position.y = a.position.y;
        b.pickup_delay = 32767;
        items = vec![a.clone(), b.clone()];
        run(&mut items);
        assert_eq!(items.len(), 2, "never-pickup items are not mergeable");
        b.pickup_delay = 40;
        items = vec![a, b];
        run(&mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].pickup_delay, 40);
    }

    #[test]
    fn capacity_caps_at_registry_max_and_remembers_partial_donor() {
        let mut a = item(1, "minecraft:stick", 63);
        a.tick_count = 40;
        let b = item(2, "minecraft:stick", 2);
        let mut items = vec![a, b];
        run(&mut items);
        assert_eq!(items.len(), 2);
        assert_eq!(items.iter().map(|i| i.stack.count).sum::<u32>(), 65);
        assert_eq!(items.iter().map(|i| i.stack.count).max(), Some(64));
        let mut a = item(1, "minecraft:snowball", 15);
        a.tick_count = 40;
        items = vec![a, item(2, "minecraft:snowball", 3)];
        run(&mut items);
        assert_eq!(items.len(), 2);
        assert_eq!(items.iter().map(|i| i.stack.count).max(), Some(16));
        let mut sword = item(3, "minecraft:diamond_sword", 1);
        sword.tick_count = 40;
        items = vec![sword, item(4, "minecraft:diamond_sword", 1)];
        run(&mut items);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn dormant_chunk_item_does_not_initiate_a_frozen_tick_40_merge() {
        let mut frozen = item(1, "minecraft:stick", 1);
        frozen.tick_count = 40;
        let mut other = item(2, "minecraft:stick", 8);
        other.tick_count = 1;
        let mut items = vec![frozen, other];
        merge_nearby_items(&mut items, |_| 64, |item| item.id != 1);
        assert_eq!(
            items.len(),
            2,
            "unticked item must not scan at tick 40 again"
        );
        items[1].tick_count = 40;
        merge_nearby_items(&mut items, |_| 64, |item| item.id != 1);
        assert_eq!(
            items.len(),
            1,
            "active item can merge a nearby dormant candidate"
        );
        assert_eq!(items[0].stack.count, 9);
    }

    #[test]
    fn actual_item_physics_tick_preserves_unlimited_age_and_advances_extended_age() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.local/horizon-client/26.3");
        if !root.join("cached-collision.nbt").exists() {
            return; // Same optional prepared fixture as native world tests.
        }
        let assets = crate::assets::Assets::load(&root).unwrap();
        let render = assets.reload(&root).unwrap();
        let facts = BlockFacts::load(&root, assets.registry).unwrap();
        let collision = crate::collisions::load(&root, assets.registry).unwrap();
        let world = World::test_scene(&assets, &render, 3).unwrap();
        let mut forever = item(1, "minecraft:stick", 1);
        forever.age = ItemEntity::UNLIMITED_AGE;
        forever.position.y = 64.0;
        assert!(forever.tick(&world, &facts, &collision));
        assert!(forever.tick(&world, &facts, &collision));
        assert_eq!(forever.age, ItemEntity::UNLIMITED_AGE);
        assert_eq!(forever.tick_count, 2);
        let mut extended = item(2, "minecraft:stick", 1);
        extended.age = (-6000_i32) as u32;
        assert!(extended.tick(&world, &facts, &collision));
        assert_eq!(extended.age as i32, -5999);
        let mut ordinary = item(3, "minecraft:stick", 1);
        ordinary.age = 5999;
        assert!(!ordinary.tick(&world, &facts, &collision));
        assert_eq!(ordinary.age, 6000);
    }

    #[test]
    fn signed_extended_age_merges_into_older_item_but_unlimited_age_cannot_merge() {
        let mut regular = item(1, "minecraft:stick", 8);
        regular.tick_count = 40;
        let mut extended = item(2, "minecraft:stick", 1);
        extended.age = (-6000_i32) as u32;
        let mut items = vec![regular, extended];
        run(&mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].age as i32, -6000);
        assert_eq!(items[0].stack.count, 9);
        let mut never = item(3, "minecraft:stick", 1);
        never.tick_count = 40;
        never.age = ItemEntity::UNLIMITED_AGE;
        let mut items = vec![never, item(4, "minecraft:stick", 8)];
        run(&mut items);
        assert_eq!(items.len(), 2, "unlimited-age item cannot merge in Java");
    }

    #[test]
    fn no_merge_across_distant_items_or_already_aged_out() {
        let mut a = item(1, "minecraft:stick", 1);
        a.tick_count = 40;
        let mut b = item(2, "minecraft:stick", 1);
        b.position.z += 5.0;
        let mut items = vec![a.clone(), b];
        run(&mut items);
        assert_eq!(items.len(), 2);
        a.age = 6000;
        items = vec![a, item(2, "minecraft:stick", 1)];
        run(&mut items);
        assert_eq!(items.len(), 2);
    }

    /// Deterministic load distributions for comparing the old O(n²) scan to
    /// the candidate nearby-cell search under precisely the same items.
    fn merge_load(count: usize, scene: &str) -> Vec<ItemEntity> {
        let mut seed = 0x1255_4ade_96fa_1723u64;
        let mut rand = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        (0..count)
            .map(|i| {
                let x = match scene {
                    "sparse" => (i % 64) as f64 * 3.0,
                    "clusters" => (i / 24) as f64 * 5.0 + (rand() % 8) as f64 * 0.07,
                    _ => (rand() % 9) as f64 * 0.07,
                };
                let z = match scene {
                    "sparse" => (i / 64) as f64 * 3.0,
                    "clusters" => (rand() % 8) as f64 * 0.07,
                    _ => (rand() % 9) as f64 * 0.07,
                };
                let mut out = item(
                    i as u32 + 1,
                    match i % 9 {
                        0 => "minecraft:snowball",
                        1 | 2 => "minecraft:stone",
                        _ => "minecraft:stick",
                    },
                    (rand() % 16 + 1) as u32,
                );
                out.position = Vec3::new(x + 0.5, 65.0 + (rand() % 5) as f64 * 0.06, z + 0.5);
                out.previous = out.position;
                out.tick_count = 40;
                out.age = 50;
                out.target = match i % 6 {
                    0 => Some([1, 0, 0, 0]),
                    1 => Some([2, 0, 0, 0]),
                    _ => None,
                };
                out
            })
            .collect()
    }

    #[test]
    fn spatial_index_matches_original_scan_on_randomized_physics_and_metadata() {
        let mut seed = 0x19af_6023_d11e_7502u64;
        let mut rand = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for scene in ["sparse", "clusters", "dense"] {
            for count in [2usize, 3, 8, 24, 64, 129, 256] {
                for repetition in 0..10 {
                    let mut source = merge_load(count, scene);
                    for item in &mut source {
                        // Exercise real-life nondefault merge exclusions,
                        // special ages, partial stacks and both active/inactive initiators.
                        item.tick_count = match rand() % 6 {
                            0 => 2,
                            1 => 39,
                            2 => 40,
                            _ => 80,
                        };
                        if rand() % 5 == 0 {
                            item.previous.x -= 1.0;
                        }
                        item.age = match rand() % 30 {
                            0 => ItemEntity::UNLIMITED_AGE,
                            1 => (-6000i32) as u32,
                            2 => 5999,
                            _ => 5 + rand() as u32 % 200,
                        };
                        item.pickup_delay = match rand() % 25 {
                            0 => u32::MAX,
                            1 => 32767,
                            _ => (rand() % 100) as u32,
                        };
                        if rand() % 8 == 0 {
                            item.stack.damage = 1;
                        }
                        if rand() % 13 == 0 {
                            item.stack.data_mut().repair_cost = 2;
                        }
                        if rand() % 10 == 0 {
                            item.stack.count = 64;
                        }
                        item.position.x += (rand() % 5) as f64 * 0.002;
                        item.position.z += (rand() % 5) as f64 * 0.002;
                    }
                    let mut baseline = source.clone();
                    let active = |it: &ItemEntity| it.id % 13 != 0;
                    merge_nearby_items_scan_reference(
                        &mut baseline,
                        |id| if id == "minecraft:snowball" { 16 } else { 64 },
                        active,
                    );
                    merge_nearby_items(
                        &mut source,
                        |id| if id == "minecraft:snowball" { 16 } else { 64 },
                        active,
                    );
                    assert_eq!(
                        format!("{source:?}"),
                        format!("{baseline:?}"),
                        "merge mismatch scene={scene} n={count} sample={repetition}"
                    );
                }
            }
        }
    }

    #[test]
    fn merge_preflight_never_skips_the_first_due_item_or_frozen_entity() {
        for scene in ["sparse", "clusters", "dense"] {
            let mut items = merge_load(48, scene);
            for entity in &mut items {
                entity.tick_count = 39;
                entity.previous = entity.position;
            }
            let before = items.clone();
            merge_nearby_items(&mut items, |_| 64, |_| true);
            assert_eq!(format!("{items:?}"), format!("{before:?}"));
            // The only due item is deliberately late in the entity list.
            items[47].tick_count = 40;
            let mut baseline = items.clone();
            merge_nearby_items_scan_reference(&mut baseline, |_| 64, |_| true);
            merge_nearby_items(&mut items, |_| 64, |_| true);
            assert_eq!(
                format!("{items:?}"),
                format!("{baseline:?}"),
                "scene={scene}"
            );
            // If its chunk is not ticking, it must not initiate a scan.
            let mut dormant = before;
            dormant[47].tick_count = 40;
            let frozen = dormant.clone();
            merge_nearby_items(&mut dormant, |_| 64, |e| e.id != 48);
            assert_eq!(format!("{dormant:?}"), format!("{frozen:?}"));
        }
    }

    #[test]
    #[ignore = "manual idle-tick benchmark for assessing avoidable spatial index builds"]
    fn item_merge_unscheduled_tick_benchmark() {
        use std::hint::black_box;
        for scene in ["sparse", "clusters", "dense"] {
            for count in [64usize, 256, 1024, 4096] {
                let mut source = merge_load(count, scene);
                for item in &mut source {
                    item.tick_count = 39;
                    item.previous = item.position;
                }
                let rounds = if count >= 4096 { 12 } else { 40 };
                let start = std::time::Instant::now();
                for _ in 0..rounds {
                    let mut items = source.clone();
                    merge_nearby_items(&mut items, |_| 64, |_| true);
                    assert_eq!(items.len(), count);
                    black_box(&items);
                }
                println!(
                    "ITEM_MERGE_IDLE scene={scene} n={count} ns_per_iter={}",
                    start.elapsed().as_nanos() / rounds as u128
                );
            }
        }
    }

    #[test]
    #[ignore = "manual same-process A/B benchmark; not a frame-rate measurement"]
    fn item_merge_old_vs_indexed_benchmark() {
        use std::hint::black_box;
        for (scene_index, scene) in ["sparse", "clusters", "dense"].iter().enumerate() {
            for count in [64usize, 256, 1024] {
                let source = merge_load(count, scene);
                let rounds = if count >= 1024 { 12 } else { 48 };
                let mut elapsed = [0u128; 2];
                let mut expected = source.clone();
                merge_nearby_items_scan_reference(&mut expected, |_| 64, |_| true);
                for cycle in 0..2 {
                    for pass in 0..2 {
                        let method = (pass + cycle + scene_index) % 2;
                        let start = std::time::Instant::now();
                        for _ in 0..rounds {
                            let mut items = source.clone();
                            if method == 0 {
                                merge_nearby_items_scan_reference(&mut items, |_| 64, |_| true);
                            } else {
                                merge_nearby_items(&mut items, |_| 64, |_| true);
                            }
                            black_box(&items);
                        }
                        elapsed[method] += start.elapsed().as_nanos();
                    }
                }
                println!(
                    "ITEM_MERGE_COMPARE scene={scene} n={count} scan_ns={} hybrid_ns={} survivors={}",
                    elapsed[0] / (rounds * 2) as u128,
                    elapsed[1] / (rounds * 2) as u128,
                    expected.len()
                );
            }
        }
    }

    #[test]
    #[ignore = "manual benchmark; compare identical workloads before/after optimization"]
    fn item_merge_dense_cluster_benchmark() {
        use std::hint::black_box;
        for scene in ["sparse", "clusters", "dense"] {
            for count in [64usize, 256, 1024] {
                let source = merge_load(count, scene);
                let iterations = if count >= 1024 { 4 } else { 12 };
                let start = std::time::Instant::now();
                let mut survivors = 0;
                for _ in 0..iterations {
                    let mut items = source.clone();
                    run(&mut items);
                    survivors = items.len();
                    black_box(&items);
                }
                let elapsed = start.elapsed();
                println!(
                    "ITEM_MERGE_BENCH scene={scene} n={count} iterations={iterations} ns_per_iter={} survivors={survivors}",
                    elapsed.as_nanos() / iterations as u128
                );
            }
        }
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    fn item(id: &str, count: u32, x: f64, age: u32) -> ItemEntity {
        let at = Vec3::new(x, 64.0, 0.5);
        ItemEntity {
            id: 0,
            thrower: None,
            target: None,
            stack: ItemStack {
                item: std::sync::Arc::from(id),
                count,
                damage: 0,
                extra: None,
            },
            health: 5,
            fire_ticks: 0,
            position: at,
            previous: at,
            velocity: Vec3::ZERO,
            on_ground: true,
            age,
            pickup_delay: 0,
            bob_offset: 0.0,
            tick_count: age,
        }
    }
    fn max_stack(stack: &ItemStack) -> u32 {
        if &*stack.item == "minecraft:ender_pearl" {
            16
        } else {
            64
        }
    }
    /// Merges around `items[i]` as its tick would; returns the counts left.
    fn merge(items: &mut [ItemEntity], i: usize) -> Vec<Option<u32>> {
        let mut removed = vec![false; items.len()];
        let mut cells = ItemCells::new(items);
        merge_with_neighbours(items, &mut removed, &mut cells, i, &max_stack);
        items
            .iter()
            .zip(removed)
            .map(|(item, gone)| (!gone).then_some(item.stack.count))
            .collect()
    }

    #[test]
    fn the_smaller_stack_goes_into_the_larger() {
        let mut items = [
            item("minecraft:sand", 3, 0.5, 41),
            item("minecraft:sand", 5, 1.2, 7),
        ];
        assert_eq!(merge(&mut items, 0), [None, Some(8)]);
        // ItemEntity.merge keeps the younger age; the ticking item's is
        // still the one before its tick.
        assert_eq!(items[1].age, 7);
        let mut items = [
            item("minecraft:sand", 5, 0.5, 41),
            item("minecraft:sand", 3, 1.2, 7),
        ];
        assert_eq!(merge(&mut items, 0), [Some(8), None]);
        assert_eq!(items[0].age, 8, "age 7, plus the rest of this tick");
    }

    #[test]
    fn items_merge_only_within_half_a_block_sideways() {
        // Boxes 0.25 wide grown by 0.5: centres closer than 0.75 meet.
        let mut items = [
            item("minecraft:sand", 1, 0.5, 1),
            item("minecraft:sand", 1, 1.24, 1),
        ];
        assert_eq!(merge(&mut items, 0), [None, Some(2)]);
        let mut items = [
            item("minecraft:sand", 1, 0.5, 1),
            item("minecraft:sand", 1, 1.26, 1),
        ];
        assert_eq!(merge(&mut items, 0), [Some(1), Some(1)]);
        let mut items = [
            item("minecraft:sand", 1, 0.5, 1),
            item("minecraft:sand", 1, 0.5, 1),
        ];
        items[1].position.y += 0.3;
        assert_eq!(
            merge(&mut items, 0),
            [Some(1), Some(1)],
            "no vertical growth"
        );
    }

    #[test]
    fn different_or_full_stacks_stay_apart() {
        let mut items = [
            item("minecraft:sand", 1, 0.5, 1),
            item("minecraft:gravel", 1, 0.5, 1),
        ];
        assert_eq!(merge(&mut items, 0), [Some(1), Some(1)]);
        let mut items = [
            item("minecraft:sand", 60, 0.5, 1),
            item("minecraft:sand", 10, 0.5, 1),
        ];
        assert_eq!(
            merge(&mut items, 0),
            [Some(60), Some(10)],
            "70 is over a stack"
        );
        let mut items = [
            item("minecraft:ender_pearl", 10, 0.5, 1),
            item("minecraft:ender_pearl", 6, 0.5, 1),
        ];
        assert_eq!(merge(&mut items, 0), [Some(16), None]);
        let mut items = [
            item("minecraft:ender_pearl", 16, 0.5, 1),
            item("minecraft:ender_pearl", 1, 0.5, 1),
        ];
        assert_eq!(
            merge(&mut items, 0),
            [Some(16), Some(1)],
            "a full stack doesn't merge"
        );
    }

    #[test]
    fn unmergeable_items_are_skipped() {
        let mut items = [
            item("minecraft:sand", 1, 0.5, 1),
            item("minecraft:sand", 1, 0.5, 1),
        ];
        items[1].pickup_delay = u32::MAX;
        assert_eq!(merge(&mut items, 0), [Some(1), Some(1)]);
        // At age 6000 the other item's lifetime is over; this one's tick
        // started at 5999, so it may still merge into a live item.
        let mut items = [
            item("minecraft:sand", 1, 0.5, 6000),
            item("minecraft:sand", 1, 0.5, 6000),
        ];
        assert_eq!(merge(&mut items, 0), [Some(1), Some(1)]);
        items[1].age = 10;
        assert_eq!(merge(&mut items, 0), [None, Some(2)]);
        assert_eq!(items[1].age, 10);
    }

    #[test]
    fn a_stack_takes_its_neighbours_in_spawn_order_until_full() {
        let mut items = [
            item("minecraft:sand", 40, 0.5, 1),
            item("minecraft:sand", 20, 0.7, 1),
            item("minecraft:sand", 10, 0.3, 1),
            item("minecraft:sand", 3, 0.4, 1),
        ];
        // 40 + 20 = 60; 60 + 10 is over 64; then 3 more.
        assert_eq!(merge(&mut items, 0), [Some(63), None, Some(10), None]);
        let mut items = [
            item("minecraft:sand", 1, 0.5, 1),
            item("minecraft:sand", 1, 0.5, 1),
        ];
        items[1].pickup_delay = 30;
        assert_eq!(merge(&mut items, 0), [None, Some(2)]);
        assert_eq!(items[1].pickup_delay, 30, "the longer pickup delay stays");
    }

    #[test]
    fn the_cells_follow_moves() {
        let mut items = [
            item("minecraft:sand", 1, 0.5, 1),
            item("minecraft:sand", 1, 5.5, 1),
        ];
        let mut removed = [false; 2];
        let mut cells = ItemCells::new(&items);
        let from = items[1].position;
        items[1].position.x = 0.9;
        cells.moved(1, from, items[1].position);
        merge_with_neighbours(&mut items, &mut removed, &mut cells, 0, &max_stack);
        assert_eq!(removed, [true, false]);
        assert_eq!(items[1].stack.count, 2);
    }
}
