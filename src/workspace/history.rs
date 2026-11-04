use std::path::PathBuf;

/// A file and its zero-based editor position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
}

/// Explicit navigation history for one Window.
#[derive(Default)]
pub struct History {
    back: Vec<Place>,
    forward: Vec<Place>,
}

impl History {
    /// Record the place a jump started from; keep the most recent 50 places.
    pub fn push(&mut self, from: Place) {
        self.forward.clear();
        if self.back.last() == Some(&from) {
            return;
        }
        if self.back.len() == 50 {
            self.back.remove(0);
        }
        self.back.push(from);
    }

    pub fn back(&mut self, current: Place) -> Option<Place> {
        let place = self.back.pop()?;
        self.forward.push(current);
        Some(place)
    }

    pub fn forward(&mut self, current: Place) -> Option<Place> {
        let place = self.forward.pop()?;
        self.back.push(current);
        Some(place)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(path: &str, line: usize, column: usize) -> Place {
        Place {
            path: PathBuf::from(path),
            line,
            column,
        }
    }

    #[test]
    fn back_and_forward_restore_exact_places_and_new_push_discards_forward() {
        let a = place("a.py", 2, 7);
        let b = place("b.py", 4, 1);
        let c = place("c.py", 8, 3);
        let mut history = History::default();
        history.push(a.clone());
        assert_eq!(history.back(b.clone()), Some(a.clone()));
        assert_eq!(history.forward(a.clone()), Some(b.clone()));
        assert_eq!(history.back(b), Some(a));
        history.push(c);
        assert_eq!(history.forward(place("d.py", 1, 0)), None);
    }

    #[test]
    fn ends_are_noops_and_duplicate_push_is_ignored() {
        let a = place("a.py", 2, 7);
        let mut history = History::default();
        assert_eq!(history.back(a.clone()), None);
        assert_eq!(history.forward(a.clone()), None);
        history.push(a.clone());
        history.push(a.clone());
        assert_eq!(history.back(place("b.py", 0, 0)), Some(a));
        assert_eq!(history.back(place("c.py", 0, 0)), None);
    }
}
