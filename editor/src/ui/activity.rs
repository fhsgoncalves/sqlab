use gpui::Context;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActivityId(usize);

#[derive(Clone, Debug)]
struct Activity {
    id: ActivityId,
    label: String,
}

pub struct ActivityTracker {
    next_id: usize,
    activities: Vec<Activity>,
}

impl ActivityTracker {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            activities: Vec::new(),
        }
    }

    pub fn begin(&mut self, label: impl Into<String>, cx: &mut Context<Self>) -> ActivityId {
        let id = ActivityId(self.next_id);
        self.next_id += 1;
        self.activities.push(Activity {
            id,
            label: label.into(),
        });
        cx.notify();
        id
    }

    pub fn finish(&mut self, id: ActivityId, cx: &mut Context<Self>) {
        self.activities.retain(|activity| activity.id != id);
        cx.notify();
    }

    pub fn is_busy(&self) -> bool {
        !self.activities.is_empty()
    }

    pub fn label(&self) -> &str {
        self.activities
            .last()
            .map(|activity| activity.label.as_str())
            .unwrap_or("Ready")
    }

    pub fn count(&self) -> usize {
        self.activities.len()
    }
}
