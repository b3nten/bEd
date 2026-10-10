//! Workspace areas form a complete rectangular tiling, independent of tab content.
//! Integer workspace coordinates keep shared borders exactly coincident.

use serde_json::{Value, json};

pub const EXTENT: i32 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rect {
    pub min: [i32; 2],
    pub max: [i32; 2],
}

impl Rect {
    pub fn contains(self, point: [i32; 2]) -> bool {
        (0..2).all(|axis| point[axis] >= self.min[axis] && point[axis] < self.max[axis])
    }

    #[cfg(test)]
    pub fn union(self, other: Self) -> Option<Self> {
        for axis in 0..2 {
            let perpendicular = axis ^ 1;
            if self.min[perpendicular] == other.min[perpendicular]
                && self.max[perpendicular] == other.max[perpendicular]
                && (self.max[axis] == other.min[axis] || other.max[axis] == self.min[axis])
            {
                return Some(Self {
                    min: [self.min[0].min(other.min[0]), self.min[1].min(other.min[1])],
                    max: [self.max[0].max(other.max[0]), self.max[1].max(other.max[1])],
                });
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub id: u32,
    pub rect: Rect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub areas: Vec<Area>,
    next_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockPlacement {
    Replace,
    /// Fraction of the target dimension occupied by the moved source area.
    Split {
        axis: usize,
        high: bool,
        fraction: i32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockPlan {
    pub layout: Layout,
    /// Removed original area IDs map directly to surviving area IDs.
    pub remap: Vec<(u32, u32)>,
}

struct Hole {
    rect: Rect,
    exclude: Option<u32>,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            areas: vec![Area {
                id: 1,
                rect: Rect {
                    min: [0, 0],
                    max: [EXTENT, EXTENT],
                },
            }],
            next_id: 2,
        }
    }
}

impl Layout {
    /// Construct a tiling at a persistence or migration boundary. Internal
    /// transitions preserve the same invariant without repeating validation.
    pub fn from_areas(areas: Vec<Area>) -> Result<Self, &'static str> {
        if areas.is_empty() || areas.len() > 4096 {
            return Err("Missing or oversized workspace tiling");
        }
        let next_id = areas
            .iter()
            .map(|area| area.id)
            .max()
            .unwrap()
            .checked_add(1)
            .ok_or("Workspace area IDs exhausted")?;
        let layout = Self { areas, next_id };
        if !layout.valid([1, 1]) {
            return Err("Invalid workspace tiling geometry");
        }
        Ok(layout)
    }

    pub fn to_value(&self) -> Value {
        json!({
            "version": 1,
            "next_id": self.next_id,
            "areas": self.areas.iter().map(|area| json!({
                "id": area.id,
                "min": area.rect.min,
                "max": area.rect.max,
            })).collect::<Vec<_>>()
        })
    }

    /// Validate externally stored geometry once before admitting it into the
    /// live workspace. Keep the allocation counter so deleted IDs stay retired.
    pub fn from_value(value: &Value) -> Result<Self, &'static str> {
        if value.get("version").and_then(Value::as_u64) != Some(1) {
            return Err("Unsupported workspace tiling version");
        }
        let next_id = value
            .get("next_id")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|&id| id > 1)
            .ok_or("Invalid workspace area allocation counter")?;
        let values = value
            .get("areas")
            .and_then(Value::as_array)
            .filter(|areas| !areas.is_empty() && areas.len() <= 4096)
            .ok_or("Missing or oversized workspace tiling")?;
        let mut areas = Vec::with_capacity(values.len());
        for value in values {
            let id = value
                .get("id")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or("Invalid workspace area ID")?;
            let coordinate = |key| -> Result<[i32; 2], &'static str> {
                let values = value
                    .get(key)
                    .and_then(Value::as_array)
                    .filter(|values| values.len() == 2)
                    .ok_or("Invalid workspace area coordinates")?;
                let mut pair = [0; 2];
                for axis in 0..2 {
                    pair[axis] = values[axis]
                        .as_i64()
                        .and_then(|value| i32::try_from(value).ok())
                        .ok_or("Invalid workspace area coordinates")?;
                }
                Ok(pair)
            };
            areas.push(Area {
                id,
                rect: Rect {
                    min: coordinate("min")?,
                    max: coordinate("max")?,
                },
            });
        }
        let layout = Self { areas, next_id };
        if !layout.valid([1, 1]) {
            return Err("Invalid workspace tiling geometry");
        }
        Ok(layout)
    }

    pub fn area(&self, id: u32) -> Area {
        *self
            .areas
            .iter()
            .find(|area| area.id == id)
            .expect("live area ID")
    }

    pub fn split(&mut self, id: u32, axis: usize, at: i32, minimum: i32) -> Option<u32> {
        assert!(axis < 2 && minimum > 0, "valid split axis and minimum");
        let index = self
            .areas
            .iter()
            .position(|area| area.id == id)
            .expect("live area ID");
        let rect = self.areas[index].rect;
        if rect.max[axis] - rect.min[axis] < minimum * 2 {
            return None;
        }
        let next_id = self.next_id.checked_add(1)?;
        let at = at.clamp(rect.min[axis] + minimum, rect.max[axis] - minimum);
        let mut second = rect;
        self.areas[index].rect.max[axis] = at;
        second.min[axis] = at;
        let new_id = self.next_id;
        self.next_id = next_id;
        self.areas.push(Area {
            id: new_id,
            rect: second,
        });
        Some(new_id)
    }

    /// Remove one footprint and let its neighbors fill the gap. Closing does
    /// not depend on the current native window's minimum panel size.
    pub fn close_area(&mut self, id: u32) -> bool {
        if self.areas.len() == 1 {
            return false;
        }
        let Some(index) = self.areas.iter().position(|area| area.id == id) else {
            return false;
        };
        let mut layout = self.clone();
        let footprint = layout.areas.remove(index).rect;
        layout
            .close_holes(
                vec![Hole {
                    rect: footprint,
                    exclude: None,
                }],
                [1, 1],
            )
            .expect("closing an area fills its footprint without removing neighbors");
        *self = layout;
        true
    }

    /// Direct joins require matching full edges. Interactive partial joins
    /// use the atomic plan below, which also resolves rectangular remainders.
    #[cfg(test)]
    pub fn join(&mut self, keep: u32, remove: u32) -> bool {
        if keep == remove {
            return false;
        }
        let Some(rect) = self.area(keep).rect.union(self.area(remove).rect) else {
            return false;
        };
        self.areas
            .iter_mut()
            .find(|area| area.id == keep)
            .unwrap()
            .rect = rect;
        self.areas.retain(|area| area.id != remove);
        true
    }

    /// Extend the source across a shared border's overlapping band. When
    /// the source fits inside the target's perpendicular extent, target tails
    /// stay as areas. A trimmed source instead closes all remainders, following
    /// the same gap-filling rules used by area relocation.
    pub fn plan_join(&self, source: u32, target: u32, minimum: [i32; 2]) -> Option<DockPlan> {
        if source == target || !self.valid(minimum) {
            return None;
        }
        let source_index = self.areas.iter().position(|area| area.id == source)?;
        let target_index = self.areas.iter().position(|area| area.id == target)?;
        let source_rect = self.areas[source_index].rect;
        let target_rect = self.areas[target_index].rect;
        let axis = (0..2).find(|&axis| {
            (source_rect.max[axis] == target_rect.min[axis]
                || target_rect.max[axis] == source_rect.min[axis])
                && source_rect.min[axis ^ 1] < target_rect.max[axis ^ 1]
                && target_rect.min[axis ^ 1] < source_rect.max[axis ^ 1]
        })?;
        let perpendicular = axis ^ 1;
        let band = [
            source_rect.min[perpendicular].max(target_rect.min[perpendicular]),
            source_rect.max[perpendicular].min(target_rect.max[perpendicular]),
        ];
        if band[1] - band[0] < minimum[perpendicular] {
            return None;
        }
        let mut layout = self.clone();
        let mut expanded = source_rect;
        expanded.min[axis] = source_rect.min[axis].min(target_rect.min[axis]);
        expanded.max[axis] = source_rect.max[axis].max(target_rect.max[axis]);
        expanded.min[perpendicular] = band[0];
        expanded.max[perpendicular] = band[1];
        layout.areas[source_index].rect = expanded;
        let mut target_tails = Vec::new();
        let mut holes = Vec::new();
        let source_trimmed =
            source_rect.min[perpendicular] != band[0] || source_rect.max[perpendicular] != band[1];
        for (id, rect) in [(source, source_rect), (target, target_rect)] {
            let mut low = rect;
            low.max[perpendicular] = band[0];
            let mut high = rect;
            high.min[perpendicular] = band[1];
            for tail in [low, high] {
                let length = tail.max[perpendicular] - tail.min[perpendicular];
                if length == 0 {
                    continue;
                }
                if length < minimum[perpendicular] {
                    return None;
                }
                if source_trimmed {
                    holes.push(Hole {
                        rect: tail,
                        exclude: Some(source),
                    });
                } else {
                    assert_eq!(id, target, "an untrimmed source has no tails");
                    target_tails.push(tail);
                }
            }
        }
        let remap = if source_trimmed || target_tails.is_empty() {
            layout.areas.retain(|area| area.id != target);
            vec![(target, source)]
        } else {
            // Keep the target's content in its larger remaining area, with a
            // stable low-side tie. Any second tail starts as an empty area;
            // layout geometry does not attempt to clone editor state.
            let retained = if target_tails.len() == 2
                && target_tails[1].max[perpendicular] - target_tails[1].min[perpendicular]
                    > target_tails[0].max[perpendicular] - target_tails[0].min[perpendicular]
            {
                1
            } else {
                0
            };
            layout.areas[target_index].rect = target_tails.remove(retained);
            for rect in target_tails {
                let id = layout.next_id;
                layout.next_id = layout.next_id.checked_add(1)?;
                layout.areas.push(Area { id, rect });
            }
            Vec::new()
        };
        layout.close_holes(holes, minimum)?;
        Some(DockPlan { layout, remap })
    }

    /// Move the source into a target slot, then close its old footprint. All
    /// work occurs on a copy, so an impossible minimum size never changes the
    /// live tiling. Content relocation is returned as data alongside geometry.
    pub fn plan_dock(
        &self,
        source: u32,
        target: u32,
        placement: DockPlacement,
        minimum: [i32; 2],
    ) -> Option<DockPlan> {
        if source == target || !self.valid(minimum) {
            return None;
        }
        let source_index = self.areas.iter().position(|area| area.id == source)?;
        let target_index = self.areas.iter().position(|area| area.id == target)?;
        let footprint = self.areas[source_index].rect;
        let target_rect = self.areas[target_index].rect;
        let mut layout = self.clone();
        let remap = match placement {
            DockPlacement::Replace => {
                layout.areas[source_index].rect = target_rect;
                layout.areas.retain(|area| area.id != target);
                vec![(target, source)]
            }
            DockPlacement::Split {
                axis,
                high,
                fraction,
            } => {
                if axis >= 2 || !(0..=EXTENT).contains(&fraction) {
                    return None;
                }
                let length = target_rect.max[axis] - target_rect.min[axis];
                if length < minimum[axis] * 2 {
                    return None;
                }
                let occupied = (i64::from(length) * i64::from(fraction) / i64::from(EXTENT)) as i32;
                let occupied = occupied.clamp(minimum[axis], length - minimum[axis]);
                let mut destination = target_rect;
                let mut remainder = target_rect;
                if high {
                    destination.min[axis] = target_rect.max[axis] - occupied;
                    remainder.max[axis] = destination.min[axis];
                } else {
                    destination.max[axis] = target_rect.min[axis] + occupied;
                    remainder.min[axis] = destination.max[axis];
                }
                layout.areas[source_index].rect = destination;
                layout.areas[target_index].rect = remainder;
                Vec::new()
            }
        };

        // Splitting precedes source closure. The footprint is a hole,
        // rather than another content area that could absorb a neighbor.
        layout.close_holes(
            vec![Hole {
                rect: footprint,
                exclude: None,
            }],
            minimum,
        )?;
        assert!(
            remap
                .iter()
                .all(|(_, destination)| layout.areas.iter().any(|area| area.id == *destination)),
            "content remaps directly to surviving areas"
        );
        Some(DockPlan { layout, remap })
    }

    fn close_holes(&mut self, mut holes: Vec<Hole>, minimum: [i32; 2]) -> Option<()> {
        // Every cut uses an existing boundary coordinate. Refuse a repeating
        // state or an unresolved plan after four passes per coordinate-grid
        // cell, rather than allowing an unsuccessful greedy closure to hang a
        // mouse preview. Rejection leaves the original layout unchanged.
        let mut cuts = [Vec::new(), Vec::new()];
        for rect in self
            .areas
            .iter()
            .map(|area| area.rect)
            .chain(holes.iter().map(|hole| hole.rect))
        {
            for (axis, cuts) in cuts.iter_mut().enumerate() {
                cuts.extend([rect.min[axis], rect.max[axis]]);
            }
        }
        for axis in &mut cuts {
            axis.sort_unstable();
            axis.dedup();
        }
        let limit = (cuts[0].len() - 1)
            .checked_mul(cuts[1].len() - 1)?
            .checked_mul(4)?;
        let mut filled = 0;
        let mut seen = std::collections::HashSet::new();
        while !holes.is_empty() {
            let state = (
                self.areas
                    .iter()
                    .map(|area| (area.id, area.rect))
                    .collect::<Vec<_>>(),
                holes
                    .iter()
                    .map(|hole| (hole.rect, hole.exclude))
                    .collect::<Vec<_>>(),
            );
            if filled >= limit || !seen.insert(state) {
                return None;
            }
            filled += 1;
            let Hole {
                rect: hole,
                exclude,
            } = holes.pop().unwrap();
            // Among legal neighbors prefer the best shared-edge length ratio,
            // preserving list order for ties. Both the hole and neighbor may
            // need trimming to their overlapping band before expansion.
            let mut best: Option<(usize, usize, [i32; 2], [i32; 2])> = None;
            for (index, area) in self.areas.iter().enumerate() {
                if Some(area.id) == exclude {
                    continue;
                }
                let neighbor = area.rect;
                for axis in 0..2 {
                    if hole.max[axis] != neighbor.min[axis] && neighbor.max[axis] != hole.min[axis]
                    {
                        continue;
                    }
                    let perpendicular = axis ^ 1;
                    let band = [
                        hole.min[perpendicular].max(neighbor.min[perpendicular]),
                        hole.max[perpendicular].min(neighbor.max[perpendicular]),
                    ];
                    if band[1] - band[0] < minimum[perpendicular] {
                        continue;
                    }
                    let tails = [
                        band[0] - hole.min[perpendicular],
                        hole.max[perpendicular] - band[1],
                        band[0] - neighbor.min[perpendicular],
                        neighbor.max[perpendicular] - band[1],
                    ];
                    if tails
                        .into_iter()
                        .any(|length| length != 0 && length < minimum[perpendicular])
                    {
                        continue;
                    }
                    let hole_length = hole.max[perpendicular] - hole.min[perpendicular];
                    let neighbor_length = neighbor.max[perpendicular] - neighbor.min[perpendicular];
                    let alignment = [
                        hole_length.min(neighbor_length),
                        hole_length.max(neighbor_length),
                    ];
                    if best.is_none_or(|(_, _, _, old)| {
                        i64::from(alignment[0]) * i64::from(old[1])
                            > i64::from(old[0]) * i64::from(alignment[1])
                    }) {
                        best = Some((index, axis, band, alignment));
                    }
                }
            }
            let Some((index, axis, band, _)) = best else {
                // Another pending hole may need to close before this one has
                // a neighbor besides its excluded survivor. Defer it to the
                // bottom of the stack; repeated full rotations are caught by
                // the same state-cycle guard above.
                holes.insert(
                    0,
                    Hole {
                        rect: hole,
                        exclude,
                    },
                );
                continue;
            };
            let perpendicular = axis ^ 1;
            let neighbor = self.areas[index];
            let expanded = Rect {
                min: [
                    hole.min[0].min(neighbor.rect.min[0]),
                    hole.min[1].min(neighbor.rect.min[1]),
                ],
                max: [
                    hole.max[0].max(neighbor.rect.max[0]),
                    hole.max[1].max(neighbor.rect.max[1]),
                ],
            };
            self.areas[index].rect = expanded;
            self.areas[index].rect.min[perpendicular] = band[0];
            self.areas[index].rect.max[perpendicular] = band[1];

            // Neighbor overhangs also close. Its original content identity
            // stays with the expanded overlap band; there is no new view to
            // clone, and no additional original content ID is removed.
            let mut low = neighbor.rect;
            low.max[perpendicular] = band[0];
            let mut high = neighbor.rect;
            high.min[perpendicular] = band[1];
            for rect in [low, high] {
                if rect.min[perpendicular] == rect.max[perpendicular] {
                    continue;
                }
                holes.push(Hole {
                    rect,
                    exclude: Some(neighbor.id),
                });
            }

            // Close hole tails independently; they cannot join back into the
            // survivor that just filled the middle of their original footprint.
            let mut low = hole;
            low.max[perpendicular] = band[0];
            let mut high = hole;
            high.min[perpendicular] = band[1];
            for rect in [high, low] {
                if rect.min[perpendicular] != rect.max[perpendicular] {
                    holes.push(Hole {
                        rect,
                        exclude: Some(neighbor.id),
                    });
                }
            }
        }
        assert!(
            self.valid(minimum),
            "gap closure preserves a complete rectangular tiling"
        );
        Some(())
    }

    fn valid(&self, minimum: [i32; 2]) -> bool {
        if minimum
            .into_iter()
            .any(|value| value <= 0 || value > EXTENT)
        {
            return false;
        }
        let mut total = 0_i64;
        for (index, area) in self.areas.iter().enumerate() {
            let rect = area.rect;
            if area.id == 0
                || area.id >= self.next_id
                || !(0..2).all(|axis| {
                    rect.min[axis] >= 0
                        && rect.min[axis] < rect.max[axis]
                        && rect.max[axis] <= EXTENT
                        && rect.max[axis] - rect.min[axis] >= minimum[axis]
                })
            {
                return false;
            }
            total += i64::from(rect.max[0] - rect.min[0]) * i64::from(rect.max[1] - rect.min[1]);
            for other in &self.areas[index + 1..] {
                if area.id == other.id
                    || !(0..2).any(|axis| {
                        rect.max[axis] <= other.rect.min[axis]
                            || other.rect.max[axis] <= rect.min[axis]
                    })
                {
                    return false;
                }
            }
        }
        total == i64::from(EXTENT).pow(2)
    }

    /// Find the whole connected collinear border, including T junctions. The
    /// result is geometry, rather than a remembered parent split.
    pub fn border_span(&self, axis: usize, at: i32, mut span: [i32; 2]) -> [i32; 2] {
        loop {
            let previous = span;
            for area in &self.areas {
                let rect = area.rect;
                if (rect.min[axis] == at || rect.max[axis] == at)
                    && rect.min[axis ^ 1] <= span[1]
                    && rect.max[axis ^ 1] >= span[0]
                {
                    span[0] = span[0].min(rect.min[axis ^ 1]);
                    span[1] = span[1].max(rect.max[axis ^ 1]);
                }
            }
            if previous == span {
                return span;
            }
        }
    }

    pub fn move_border(
        &mut self,
        axis: usize,
        at: i32,
        seed: [i32; 2],
        to: i32,
        minimum: i32,
    ) -> i32 {
        assert!(axis < 2 && minimum > 0, "valid border axis and minimum");
        assert!(at > 0 && at < EXTENT, "workspace border cannot move");
        let span = self.border_span(axis, at, seed);
        let touches = |rect: Rect| rect.min[axis ^ 1] < span[1] && rect.max[axis ^ 1] > span[0];
        let mut limits = [0, EXTENT];
        for area in &self.areas {
            if touches(area.rect) {
                if area.rect.max[axis] == at {
                    limits[0] = limits[0].max(area.rect.min[axis] + minimum);
                }
                if area.rect.min[axis] == at {
                    limits[1] = limits[1].min(area.rect.max[axis] - minimum);
                }
            }
        }
        // A smaller native window can leave insufficient room for the minimum.
        // Preserve the current tiling until there is space to move the border.
        if limits[0] > limits[1] {
            return at;
        }
        let to = to.clamp(limits[0], limits[1]);
        for area in &mut self.areas {
            if touches(area.rect) {
                if area.rect.min[axis] == at {
                    area.rect.min[axis] = to;
                }
                if area.rect.max[axis] == at {
                    area.rect.max[axis] = to;
                }
            }
        }
        to
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistence_round_trip_keeps_deleted_area_ids_retired() {
        let mut layout = Layout::default();
        let removed = layout.split(1, 0, 5000, 500).unwrap();
        assert!(layout.join(1, removed));
        let saved = layout.to_value();
        let mut restored = Layout::from_value(&saved).unwrap();
        assert_eq!(layout, restored);
        assert_eq!(restored.split(1, 1, 5000, 500), Some(removed + 1));

        let restored = Layout::from_value(&t_layout().to_value()).unwrap();
        assert_eq!(restored, t_layout());
    }

    #[test]
    fn persistence_rejects_invalid_geometry_and_identifiers_at_the_boundary() {
        let original = t_layout().to_value();
        for (field, invalid) in [
            ("version", json!(2)),
            ("version", json!("1")),
            ("next_id", json!(3)),
            ("next_id", json!(u64::MAX)),
            ("areas", json!([])),
            ("areas", json!([null])),
        ] {
            let mut saved = original.clone();
            saved[field] = invalid;
            assert!(Layout::from_value(&saved).is_err(), "{field}");
        }
        for invalid in [
            json!({"id":0,"min":[0,0],"max":[5000,EXTENT]}),
            json!({"id":2,"min":[0,0],"max":[5000,EXTENT]}),
            json!({"id":1,"min":[0,0],"max":[4999,EXTENT]}),
            json!({"id":1,"min":[0,0],"max":[5001,EXTENT]}),
            json!({"id":1,"min":[-1,0],"max":[5000,EXTENT]}),
            json!({"id":1,"min":[0,0],"max":[5000,EXTENT+1]}),
            json!({"id":1,"min":[0,0],"max":[0,EXTENT]}),
            json!({"id":1,"min":[0,0],"max":[5000.5,EXTENT]}),
            json!({"id":1,"min":[0,0],"max":[5000,"10000"]}),
            json!({"id":1,"min":[0,0,0],"max":[5000,EXTENT]}),
        ] {
            let mut saved = original.clone();
            saved["areas"][0] = invalid;
            assert!(Layout::from_value(&saved).is_err(), "{saved}");
        }
    }

    #[test]
    fn migration_validates_tiling_before_allocating_stable_new_ids() {
        let original = t_layout();
        let mut restored = Layout::from_areas(original.areas.clone()).unwrap();
        assert_eq!(original, restored);
        assert_eq!(restored.split(1, 0, 2500, 500), Some(4));
        assert!(Layout::from_areas(Vec::new()).is_err());
        let mut duplicate = original.areas;
        duplicate[1].id = duplicate[0].id;
        assert!(Layout::from_areas(duplicate).is_err());
        assert!(
            Layout::from_areas(vec![Area {
                id: u32::MAX,
                rect: Layout::default().area(1).rect,
            }])
            .is_err()
        );
    }

    #[test]
    fn exhausted_area_ids_refuse_a_split_without_changing_geometry() {
        let mut saved = Layout::default().to_value();
        saved["next_id"] = json!(u32::MAX);
        let mut restored = Layout::from_value(&saved).unwrap();
        let before = restored.clone();
        assert_eq!(restored.split(1, 0, 5000, 500), None);
        assert_eq!(restored, before);
    }

    #[test]
    fn area_closure_fills_t_junctions_and_pinwheels_without_changing_survivor_ids() {
        let pinwheel = Layout::from_areas(
            [
                (1, [0, 0], [7000, 3000]),
                (2, [0, 3000], [3000, EXTENT]),
                (3, [3000, 3000], [7000, 7000]),
                (4, [7000, 0], [EXTENT, 7000]),
                (5, [3000, 7000], [EXTENT, EXTENT]),
            ]
            .into_iter()
            .map(|(id, min, max)| Area {
                id,
                rect: Rect { min, max },
            })
            .collect(),
        )
        .unwrap();
        for original in [t_layout(), pinwheel] {
            for swap in [false, true] {
                for mirrors in 0..4 {
                    for removed in &original.areas {
                        let mut layout = original.clone();
                        for area in &mut layout.areas {
                            area.rect = transformed(area.rect, swap, mirrors);
                        }
                        assert!(layout.close_area(removed.id));
                        assert_eq!(layout.next_id, original.next_id);
                        assert_eq!(
                            layout.areas.iter().map(|area| area.id).collect::<Vec<_>>(),
                            original
                                .areas
                                .iter()
                                .filter(|area| area.id != removed.id)
                                .map(|area| area.id)
                                .collect::<Vec<_>>()
                        );
                        valid(&layout);
                        assert_eq!(Layout::from_value(&layout.to_value()).unwrap(), layout);
                    }
                }
            }
        }
    }

    fn valid(layout: &Layout) {
        let mut total = 0_i64;
        for (index, area) in layout.areas.iter().enumerate() {
            let rect = area.rect;
            assert!((0..2).all(|axis| 0 <= rect.min[axis]
                && rect.min[axis] < rect.max[axis]
                && rect.max[axis] <= EXTENT));
            total += i64::from(rect.max[0] - rect.min[0]) * i64::from(rect.max[1] - rect.min[1]);
            for other in &layout.areas[index + 1..] {
                assert!(area.id != other.id);
                assert!((0..2).any(|axis| rect.max[axis] <= other.rect.min[axis]
                    || other.rect.max[axis] <= rect.min[axis]));
            }
        }
        assert_eq!(total, i64::from(EXTENT).pow(2));
    }

    fn t_layout() -> Layout {
        let mut layout = Layout::default();
        let right = layout.split(1, 0, 5000, 500).unwrap();
        layout.split(right, 1, 5000, 500).unwrap();
        layout
    }

    fn transformed(rect: Rect, swap: bool, mirrors: usize) -> Rect {
        let mut rect = rect;
        if swap {
            rect.min.swap(0, 1);
            rect.max.swap(0, 1);
        }
        for axis in 0..2 {
            if mirrors & (1 << axis) != 0 {
                (rect.min[axis], rect.max[axis]) =
                    (EXTENT - rect.max[axis], EXTENT - rect.min[axis]);
            }
        }
        rect
    }

    fn grid() -> (Layout, [[u32; 3]; 3]) {
        let mut layout = Layout::default();
        let middle = layout.split(1, 1, 3000, 500).unwrap();
        let bottom = layout.split(middle, 1, 7000, 500).unwrap();
        let mut cells = [[0; 3]; 3];
        for (row, first) in [1, middle, bottom].into_iter().enumerate() {
            let second = layout.split(first, 0, 3000, 500).unwrap();
            let third = layout.split(second, 0, 7000, 500).unwrap();
            cells[row] = [first, second, third];
        }
        (layout, cells)
    }

    #[test]
    fn a_short_source_extends_across_a_t_without_consuming_the_target_tail() {
        for swap in [false, true] {
            for mirrors in 0..4 {
                let mut layout = t_layout();
                for area in &mut layout.areas {
                    area.rect = transformed(area.rect, swap, mirrors);
                }
                let before = layout.clone();
                let plan = layout.plan_join(2, 1, [500, 500]).unwrap();
                assert_eq!(layout, before, "preview planning preserves the live layout");
                assert_eq!(plan.layout.areas.len(), 3);
                assert_eq!(plan.layout.next_id, layout.next_id);
                assert!(plan.remap.is_empty());
                for (id, rect) in [
                    (
                        2,
                        Rect {
                            min: [0, 0],
                            max: [EXTENT, 5000],
                        },
                    ),
                    (
                        1,
                        Rect {
                            min: [0, 5000],
                            max: [5000, EXTENT],
                        },
                    ),
                    (
                        3,
                        Rect {
                            min: [5000, 5000],
                            max: [EXTENT, EXTENT],
                        },
                    ),
                ] {
                    assert_eq!(plan.layout.area(id).rect, transformed(rect, swap, mirrors));
                }
                valid(&plan.layout);
            }
        }
    }

    #[test]
    fn two_target_tails_keep_content_in_the_larger_tail_with_a_stable_low_tie() {
        for (low, high, retained, empty) in [
            (3000, 7000, [0, 3000], [7000, EXTENT]),
            (2000, 6000, [6000, EXTENT], [0, 2000]),
            (4000, 8000, [0, 4000], [8000, EXTENT]),
        ] {
            let mut layout = Layout::default();
            let right = layout.split(1, 0, 5000, 500).unwrap();
            let source = layout.split(right, 1, low, 500).unwrap();
            let bottom = layout.split(source, 1, high, 500).unwrap();
            let before = layout.clone();
            let plan = layout.plan_join(source, 1, [500, 500]).unwrap();
            assert_eq!(layout, before);
            assert!(plan.remap.is_empty());
            assert_eq!(plan.layout.areas.len(), 5);
            assert_eq!(plan.layout.next_id, layout.next_id + 1);
            assert_eq!(
                plan.layout.area(source).rect,
                Rect {
                    min: [0, low],
                    max: [EXTENT, high]
                }
            );
            assert_eq!(
                plan.layout.area(1).rect,
                Rect {
                    min: [0, retained[0]],
                    max: [5000, retained[1]]
                }
            );
            assert_eq!(
                plan.layout.area(layout.next_id).rect,
                Rect {
                    min: [0, empty[0]],
                    max: [5000, empty[1]]
                }
            );
            assert_eq!(plan.layout.area(right), layout.area(right));
            assert_eq!(plan.layout.area(bottom), layout.area(bottom));
            valid(&plan.layout);
        }
    }

    #[test]
    fn a_trimmed_source_closes_overhangs_in_every_orientation() {
        for swap in [false, true] {
            for mirrors in 0..4 {
                for (source_low, source_high, target_low, target_high) in [
                    (0, EXTENT, 0, 5000),
                    (0, EXTENT, 3000, 7000),
                    (0, 6000, 2000, EXTENT),
                ] {
                    let mut areas = vec![
                        Area {
                            id: 1,
                            rect: Rect {
                                min: [0, source_low],
                                max: [5000, source_high],
                            },
                        },
                        Area {
                            id: 2,
                            rect: Rect {
                                min: [5000, target_low],
                                max: [EXTENT, target_high],
                            },
                        },
                    ];
                    let mut expected = Vec::new();
                    for (min, max, expected_min, expected_max) in [
                        ([0, 0], [5000, source_low], [0, 0], [EXTENT, source_low]),
                        (
                            [0, source_high],
                            [5000, EXTENT],
                            [0, source_high],
                            [EXTENT, EXTENT],
                        ),
                        (
                            [5000, 0],
                            [EXTENT, target_low],
                            [0, 0],
                            [EXTENT, target_low],
                        ),
                        (
                            [5000, target_high],
                            [EXTENT, EXTENT],
                            [0, target_high],
                            [EXTENT, EXTENT],
                        ),
                    ] {
                        if min[1] == max[1] {
                            continue;
                        }
                        let id = areas.len() as u32 + 1;
                        areas.push(Area {
                            id,
                            rect: Rect { min, max },
                        });
                        expected.push((
                            id,
                            Rect {
                                min: expected_min,
                                max: expected_max,
                            },
                        ));
                    }
                    for area in &mut areas {
                        area.rect = transformed(area.rect, swap, mirrors);
                    }
                    let layout = Layout {
                        next_id: areas.len() as u32 + 1,
                        areas,
                    };
                    let before = layout.clone();
                    let plan = layout.plan_join(1, 2, [500, 500]).unwrap();
                    assert_eq!(layout, before);
                    assert_eq!(plan.remap, vec![(2, 1)]);
                    assert_eq!(plan.layout.next_id, layout.next_id);
                    assert_eq!(plan.layout.areas.len(), layout.areas.len() - 1);
                    assert_eq!(
                        plan.layout.area(1).rect,
                        transformed(
                            Rect {
                                min: [0, source_low.max(target_low)],
                                max: [EXTENT, source_high.min(target_high)]
                            },
                            swap,
                            mirrors
                        )
                    );
                    for (id, rect) in expected {
                        assert_eq!(plan.layout.area(id).rect, transformed(rect, swap, mirrors));
                    }
                    valid(&plan.layout);
                }
            }
        }
    }

    #[test]
    fn full_edge_plans_preserve_the_source_and_remap_the_consumed_target() {
        let mut layout = Layout::default();
        let right = layout.split(1, 0, 5000, 500).unwrap();
        for (source, target) in [(1, right), (right, 1)] {
            let plan = layout.plan_join(source, target, [500, 500]).unwrap();
            assert_eq!(plan.remap, vec![(target, source)]);
            assert_eq!(
                plan.layout.areas,
                vec![Area {
                    id: source,
                    rect: Rect {
                        min: [0, 0],
                        max: [EXTENT, EXTENT]
                    }
                }]
            );
            valid(&plan.layout);
        }
    }

    #[test]
    fn partial_join_refusals_leave_the_original_layout_unchanged() {
        // Every original area satisfies the minimum. Offset horizontal
        // borders can still leave a target tail or overlap band too narrow.
        for (left_split, right_split, source, target) in [(6000, 5700, 2, 1), (5000, 4700, 4, 1)] {
            let mut layout = Layout::default();
            let right = layout.split(1, 0, 5000, 500).unwrap();
            layout.split(1, 1, left_split, 500).unwrap();
            layout.split(right, 1, right_split, 500).unwrap();
            assert!(layout.valid([500, 500]));
            let before = layout.clone();
            assert!(layout.plan_join(source, target, [500, 500]).is_none());
            assert_eq!(layout, before);
        }

        let (layout, cells) = grid();
        let before = layout.clone();
        for (source, target, minimum) in [
            (cells[0][0], cells[0][0], [500, 500]),
            (cells[0][0], cells[2][2], [500, 500]),
            (cells[0][0], cells[1][1], [500, 500]),
            (cells[0][0], 99, [500, 500]),
            (99, cells[0][0], [500, 500]),
            (cells[0][0], cells[0][1], [0, 500]),
        ] {
            assert!(layout.plan_join(source, target, minimum).is_none());
            assert_eq!(layout, before);
        }
    }

    #[test]
    fn mismatched_center_replacement_fills_both_bands_in_every_orientation() {
        for swap in [false, true] {
            for mirrors in 0..4 {
                let mut layout = t_layout();
                for area in &mut layout.areas {
                    area.rect = transformed(area.rect, swap, mirrors);
                }
                let before = layout.clone();
                let plan = layout
                    .plan_dock(1, 2, DockPlacement::Replace, [500, 500])
                    .unwrap();
                assert_eq!(layout, before, "planning never edits the live layout");
                assert_eq!(plan.layout.areas.len(), 2);
                assert_eq!(plan.remap, vec![(2, 1)]);
                assert_eq!(
                    plan.layout.area(1).rect,
                    transformed(
                        Rect {
                            min: [0, 0],
                            max: [EXTENT, 5000]
                        },
                        swap,
                        mirrors
                    )
                );
                assert_eq!(
                    plan.layout.area(3).rect,
                    transformed(
                        Rect {
                            min: [0, 5000],
                            max: [EXTENT, EXTENT]
                        },
                        swap,
                        mirrors
                    )
                );
                valid(&plan.layout);
            }
        }
    }

    #[test]
    fn edge_docking_splits_the_target_before_gap_filling_in_every_orientation() {
        for (axis, high, expected_source, expected_target) in [
            (
                0,
                false,
                Rect {
                    min: [0, 0],
                    max: [7500, 5000],
                },
                Rect {
                    min: [7500, 0],
                    max: [EXTENT, 5000],
                },
            ),
            (
                0,
                true,
                Rect {
                    min: [7500, 0],
                    max: [EXTENT, 5000],
                },
                Rect {
                    min: [0, 0],
                    max: [7500, 5000],
                },
            ),
            (
                1,
                false,
                Rect {
                    min: [0, 0],
                    max: [EXTENT, 2500],
                },
                Rect {
                    min: [0, 2500],
                    max: [EXTENT, 5000],
                },
            ),
            (
                1,
                true,
                Rect {
                    min: [0, 2500],
                    max: [EXTENT, 5000],
                },
                Rect {
                    min: [0, 0],
                    max: [EXTENT, 2500],
                },
            ),
        ] {
            for swap in [false, true] {
                for mirrors in 0..4 {
                    let mut layout = t_layout();
                    for area in &mut layout.areas {
                        area.rect = transformed(area.rect, swap, mirrors);
                    }
                    let moved_axis = if swap { axis ^ 1 } else { axis };
                    let placement = DockPlacement::Split {
                        axis: moved_axis,
                        high: high ^ (mirrors & (1 << moved_axis) != 0),
                        fraction: 5000,
                    };
                    let plan = layout.plan_dock(1, 2, placement, [500, 500]).unwrap();
                    assert!(plan.remap.is_empty());
                    assert_eq!(plan.layout.areas.len(), 3);
                    assert_eq!(
                        plan.layout.area(1).rect,
                        transformed(expected_source, swap, mirrors)
                    );
                    assert_eq!(
                        plan.layout.area(2).rect,
                        transformed(expected_target, swap, mirrors)
                    );
                    assert_eq!(
                        plan.layout.area(3).rect,
                        transformed(
                            Rect {
                                min: [0, 5000],
                                max: [EXTENT, EXTENT]
                            },
                            swap,
                            mirrors
                        )
                    );
                    valid(&plan.layout);
                }
            }
        }
    }

    #[test]
    fn nonadjacent_and_surrounded_sources_move_without_losing_original_ids() {
        let (layout, cells) = grid();
        for (source, target) in [(cells[1][1], cells[0][0]), (cells[0][0], cells[2][2])] {
            for placement in [
                DockPlacement::Replace,
                DockPlacement::Split {
                    axis: 0,
                    high: false,
                    fraction: 5000,
                },
                DockPlacement::Split {
                    axis: 1,
                    high: true,
                    fraction: 5000,
                },
            ] {
                let plan = layout
                    .plan_dock(source, target, placement, [500, 500])
                    .unwrap();
                valid(&plan.layout);
                assert!(plan.layout.areas.iter().any(|area| area.id == source));
                for original in &layout.areas {
                    let surviving = plan
                        .remap
                        .iter()
                        .find_map(|(old, new)| (*old == original.id).then_some(*new))
                        .unwrap_or(original.id);
                    assert!(plan.layout.areas.iter().any(|area| area.id == surviving));
                }
                assert_eq!(
                    plan.layout.areas.len(),
                    if placement == DockPlacement::Replace {
                        8
                    } else {
                        9
                    }
                );
            }
        }
    }

    #[test]
    fn neighbor_overhangs_close_recursively_instead_of_creating_empty_areas() {
        let layout = Layout {
            areas: vec![
                Area {
                    id: 1,
                    rect: Rect {
                        min: [0, 0],
                        max: [7000, 3000],
                    },
                },
                Area {
                    id: 2,
                    rect: Rect {
                        min: [0, 3000],
                        max: [3000, EXTENT],
                    },
                },
                Area {
                    id: 3,
                    rect: Rect {
                        min: [3000, 3000],
                        max: [7000, 7000],
                    },
                },
                Area {
                    id: 4,
                    rect: Rect {
                        min: [7000, 0],
                        max: [EXTENT, 7000],
                    },
                },
                Area {
                    id: 5,
                    rect: Rect {
                        min: [3000, 7000],
                        max: [EXTENT, EXTENT],
                    },
                },
            ],
            next_id: 6,
        };
        let plan = layout
            .plan_dock(3, 1, DockPlacement::Replace, [500, 500])
            .unwrap();
        assert_eq!(plan.layout.areas.len(), 4);
        assert_eq!(
            plan.layout.area(2).rect,
            Rect {
                min: [0, 3000],
                max: [7000, 7000]
            }
        );
        assert_eq!(
            plan.layout.area(5).rect,
            Rect {
                min: [0, 7000],
                max: [EXTENT, EXTENT]
            }
        );
        assert_eq!(plan.remap, vec![(1, 3)]);
        assert_eq!(
            plan.layout.next_id, layout.next_id,
            "gap filling does not invent content identities"
        );
        valid(&plan.layout);
    }

    #[test]
    fn a_blocked_hole_waits_for_other_pending_holes_to_close() {
        // This uneven tiling previously attempted a bottom-right tail while
        // its only live neighbor was its excluded survivor. Closing another
        // pending tail first supplies a legal neighbor for that same hole.
        let areas = [
            (1, [0, 0], [2086, EXTENT]),
            (2, [2996, 0], [EXTENT, 3548]),
            (3, [2086, 0], [2996, 6190]),
            (4, [2086, 6190], [2996, EXTENT]),
            (7, [2996, 3548], [8069, 5952]),
            (8, [2996, 7681], [EXTENT, EXTENT]),
            (9, [2996, 5952], [3734, 7681]),
            (10, [4414, 5952], [EXTENT, 7681]),
            (11, [8069, 3548], [EXTENT, 4599]),
            (12, [8069, 5210], [EXTENT, 5952]),
            (14, [8069, 4599], [EXTENT, 5210]),
            (15, [3734, 5952], [4414, 7176]),
            (18, [3734, 7176], [4414, 7681]),
        ]
        .into_iter()
        .map(|(id, min, max)| Area {
            id,
            rect: Rect { min, max },
        })
        .collect();
        let layout = Layout { areas, next_id: 22 };
        let before = layout.clone();
        let plan = layout
            .plan_dock(
                7,
                3,
                DockPlacement::Split {
                    axis: 1,
                    high: true,
                    fraction: 2111,
                },
                [1, 1],
            )
            .expect("a blocked pending hole can acquire a neighbor later");
        assert_eq!(layout, before);
        assert_eq!(plan.layout.areas.len(), layout.areas.len());
        assert!(plan.remap.is_empty());
        assert_eq!(plan.layout.next_id, layout.next_id);
        for original in &layout.areas {
            assert!(plan.layout.areas.iter().any(|area| area.id == original.id));
        }
        valid(&plan.layout);
    }

    #[test]
    fn docking_clamps_minimum_size_and_rejects_invalid_requests_atomically() {
        let layout = t_layout();
        let before = layout.clone();
        let plan = layout
            .plan_dock(
                1,
                2,
                DockPlacement::Split {
                    axis: 0,
                    high: false,
                    fraction: 0,
                },
                [500, 500],
            )
            .unwrap();
        assert_eq!(plan.layout.area(2).rect.min[0], 5500);
        valid(&plan.layout);
        for (source, target, placement, minimum) in [
            (1, 1, DockPlacement::Replace, [500, 500]),
            (1, 99, DockPlacement::Replace, [500, 500]),
            (99, 2, DockPlacement::Replace, [500, 500]),
            (1, 2, DockPlacement::Replace, [0, 500]),
            (1, 2, DockPlacement::Replace, [500, EXTENT + 1]),
            (
                1,
                2,
                DockPlacement::Split {
                    axis: 2,
                    high: false,
                    fraction: 5000,
                },
                [500, 500],
            ),
            (
                1,
                2,
                DockPlacement::Split {
                    axis: 0,
                    high: false,
                    fraction: -1,
                },
                [500, 500],
            ),
            (
                1,
                2,
                DockPlacement::Split {
                    axis: 0,
                    high: false,
                    fraction: EXTENT + 1,
                },
                [500, 500],
            ),
            (
                1,
                2,
                DockPlacement::Split {
                    axis: 0,
                    high: false,
                    fraction: 5000,
                },
                [3000, 500],
            ),
        ] {
            assert!(
                layout
                    .plan_dock(source, target, placement, minimum)
                    .is_none()
            );
            assert_eq!(layout, before);
        }
        let mut invalid = layout.clone();
        invalid.areas[0].rect.max[0] = 6000;
        assert!(
            invalid
                .plan_dock(1, 2, DockPlacement::Replace, [500, 500])
                .is_none()
        );
        invalid.areas[0].rect = Rect {
            min: [i32::MAX, 0],
            max: [i32::MIN, EXTENT],
        };
        assert!(
            invalid
                .plan_dock(1, 2, DockPlacement::Replace, [500, 500])
                .is_none()
        );
    }

    #[test]
    fn neighbors_join_across_original_split_branches() {
        let mut layout = Layout::default();
        let right = layout.split(1, 0, 5_000, 500).unwrap();
        layout.split(1, 1, 5_000, 500).unwrap();
        layout.split(right, 1, 5_000, 500).unwrap();
        assert!(layout.join(1, right));
        assert_eq!(
            layout.area(1).rect,
            Rect {
                min: [0, 0],
                max: [EXTENT, 5_000]
            }
        );
        valid(&layout);
    }

    #[test]
    fn a_join_that_would_make_an_l_shape_changes_nothing() {
        let mut layout = Layout::default();
        let right = layout.split(1, 0, 5_000, 500).unwrap();
        layout.split(1, 1, 5_000, 500).unwrap();
        let before = layout.clone();
        assert!(!layout.join(1, right));
        assert_eq!(layout, before);
    }

    #[test]
    fn connected_borders_move_together_and_clamp_to_smallest_neighbor() {
        let mut layout = Layout::default();
        let right = layout.split(1, 0, 5_000, 500).unwrap();
        let bottom_left = layout.split(1, 1, 5_000, 500).unwrap();
        layout.split(right, 1, 3_000, 500).unwrap();
        let far_right = layout.split(right, 0, 7_000, 500).unwrap();
        assert_eq!(layout.move_border(0, 5_000, [0, 5_000], 9_000, 500), 6_500);
        assert_eq!(layout.area(bottom_left).rect.max[0], 6_500);
        assert_eq!(layout.area(right).rect.min[0], 6_500);
        assert_eq!(layout.area(far_right).rect.min[0], 7_000);
        valid(&layout);
    }

    #[test]
    fn disconnected_borders_at_the_same_coordinate_remain_independent() {
        let mut layout = Layout::default();
        let middle = layout.split(1, 1, 3_000, 500).unwrap();
        let bottom = layout.split(middle, 1, 7_000, 500).unwrap();
        layout.split(1, 0, 5_000, 500).unwrap();
        layout.split(bottom, 0, 5_000, 500).unwrap();
        layout.move_border(0, 5_000, [0, 3_000], 6_000, 500);
        assert_eq!(layout.area(bottom).rect.max[0], 5_000);
        valid(&layout);
    }

    #[test]
    fn splits_honor_minimum_size_without_losing_workspace_coverage() {
        let mut layout = Layout::default();
        layout.split(1, 0, 0, 1_000).unwrap();
        let before = layout.clone();
        assert_eq!(layout.split(1, 0, 500, 1_000), None);
        assert_eq!(layout, before);
        valid(&layout);
    }

    #[test]
    fn joining_grid_cells_can_form_a_layout_without_any_full_workspace_cut() {
        let mut layout = Layout::default();
        let second_row = layout.split(1, 1, 3_000, 500).unwrap();
        let third_row = layout.split(second_row, 1, 7_000, 500).unwrap();
        let mut cells = [[0; 3]; 3];
        for (row, first) in [1, second_row, third_row].into_iter().enumerate() {
            let second = layout.split(first, 0, 3_000, 500).unwrap();
            let third = layout.split(second, 0, 7_000, 500).unwrap();
            cells[row] = [first, second, third];
        }
        assert!(layout.join(cells[0][0], cells[0][1]));
        assert!(layout.join(cells[0][2], cells[1][2]));
        assert!(layout.join(cells[2][1], cells[2][2]));
        assert!(layout.join(cells[1][0], cells[2][0]));
        assert_eq!(layout.areas.len(), 5);
        for axis in 0..2 {
            for at in [3_000, 7_000] {
                assert!(
                    layout
                        .areas
                        .iter()
                        .any(|area| area.rect.min[axis] < at && area.rect.max[axis] > at)
                );
            }
        }
        valid(&layout);
    }
}
