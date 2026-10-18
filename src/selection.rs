use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Off,
    On,
    Partial,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlanItem {
    Folder { src: String, dest: String, excludes: Vec<String> },
    Files { src_dir: String, dest: String, files: Vec<String> },
}

#[derive(Debug, Clone, Default)]
pub struct Selection {
    included: BTreeMap<String, bool>,
    excluded: BTreeSet<String>,
}

fn parent(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(parent, _)| parent)
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_under(path: &str, ancestor: &str) -> bool {
    path.len() > ancestor.len() && path.starts_with(ancestor) && path.as_bytes()[ancestor.len()] == b'/'
}

fn relative<'a>(path: &'a str, ancestor: &str) -> &'a str {
    &path[ancestor.len() + 1..]
}

impl Selection {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.included.is_empty()
    }

    pub fn clear(&mut self) {
        self.included.clear();
        self.excluded.clear();
    }

    fn decision(&self, path: &str) -> Option<bool> {
        let mut current = Some(path);
        while let Some(p) = current {
            if self.excluded.contains(p) {
                return Some(false);
            }
            if self.included.contains_key(p) {
                return Some(true);
            }
            current = parent(p);
        }
        None
    }

    fn covered_by_ancestors(&self, path: &str) -> bool {
        parent(path).is_some_and(|p| self.decision(p) == Some(true))
    }

    pub fn check(&self, path: &str) -> Check {
        if self.decision(path) == Some(true) {
            if self.excluded.iter().any(|e| is_under(e, path)) { Check::Partial } else { Check::On }
        } else if self.included.keys().any(|i| is_under(i, path)) {
            Check::Partial
        } else {
            Check::Off
        }
    }

    pub fn set(&mut self, path: &str, is_dir: bool, on: bool) {
        self.included.retain(|p, _| !is_under(p, path));
        self.excluded.retain(|p| !is_under(p, path));
        self.included.remove(path);
        self.excluded.remove(path);
        let covered = self.covered_by_ancestors(path);
        if on && !covered {
            self.included.insert(path.to_string(), is_dir);
        } else if !on && covered {
            self.excluded.insert(path.to_string());
        }
    }

    fn topmost_included_ancestor(&self, path: &str) -> Option<&str> {
        self.included
            .iter()
            .filter(|(p, is_dir)| **is_dir && is_under(path, p))
            .map(|(p, _)| p.as_str())
            .min_by_key(|p| p.len())
    }

    fn dest_for_dir(&self, path: &str) -> String {
        if path.is_empty() {
            return String::new();
        }
        match self.topmost_included_ancestor(path) {
            Some(top) => format!("{}/{}", leaf(top), relative(path, top)),
            None => leaf(path).to_string(),
        }
    }

    fn excludes_under(&self, path: &str) -> Vec<String> {
        self.excluded.iter().filter(|e| is_under(e, path)).map(|e| relative(e, path).to_string()).collect()
    }

    pub fn plan(&self) -> Vec<PlanItem> {
        let mut items = Vec::new();
        let mut files_by_dir: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for (path, &is_dir) in &self.included {
            if is_dir {
                items.push(PlanItem::Folder { src: path.clone(), dest: self.dest_for_dir(path), excludes: self.excludes_under(path) });
            } else {
                files_by_dir.entry(parent(path).unwrap_or("")).or_default().push(leaf(path).to_string());
            }
        }
        for (dir, files) in files_by_dir {
            items.push(PlanItem::Files { src_dir: dir.to_string(), dest: self.dest_for_dir(dir), files });
        }
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(src: &str, dest: &str, excludes: &[&str]) -> PlanItem {
        PlanItem::Folder { src: src.into(), dest: dest.into(), excludes: excludes.iter().map(|e| e.to_string()).collect() }
    }

    #[test]
    fn checking_a_folder_selects_everything_inside() {
        let mut s = Selection::default();
        s.set("A", true, true);
        assert_eq!(s.check("A"), Check::On);
        assert_eq!(s.check("A/B"), Check::On);
        assert_eq!(s.check("A/B/foto.jpg"), Check::On);
        assert_eq!(s.check("AB"), Check::Off);
        assert_eq!(s.check("C/A"), Check::Off);
        assert_eq!(s.plan(), vec![folder("A", "A", &[])]);
    }

    #[test]
    fn unchecking_a_child_leaves_the_parent_partial_and_records_an_exclusion() {
        let mut s = Selection::default();
        s.set("A", true, true);
        s.set("A/B", true, false);
        assert_eq!((s.check("A"), s.check("A/B"), s.check("A/B/x"), s.check("A/C")), (Check::Partial, Check::Off, Check::Off, Check::On));
        assert_eq!(s.plan(), vec![folder("A", "A", &["B"])]);
    }

    #[test]
    fn checking_only_a_child_marks_ancestors_as_partial() {
        let mut s = Selection::default();
        s.set("A/B", true, true);
        assert_eq!((s.check("A"), s.check("A/B"), s.check("A/C")), (Check::Partial, Check::On, Check::Off));
        assert_eq!(s.plan(), vec![folder("A/B", "B", &[])]);
    }

    #[test]
    fn checking_the_parent_absorbs_previous_choices_below_it() {
        let mut s = Selection::default();
        s.set("A/B", true, true);
        s.set("A/C", true, true);
        s.set("A", true, true);
        assert_eq!(s.plan(), vec![folder("A", "A", &[])]);
        s.set("A/B", true, false);
        s.set("A", true, true);
        assert_eq!(s.check("A"), Check::On);
        assert_eq!(s.plan(), vec![folder("A", "A", &[])]);
    }

    #[test]
    fn unchecking_the_root_of_a_selection_clears_it() {
        let mut s = Selection::default();
        s.set("A", true, true);
        s.set("A/B", true, false);
        s.set("A", true, false);
        assert!(s.is_empty());
        assert_eq!((s.check("A"), s.check("A/B")), (Check::Off, Check::Off));
        assert!(s.plan().is_empty());
    }

    #[test]
    fn single_files_are_grouped_by_their_folder() {
        let mut s = Selection::default();
        s.set("Imágenes/Toledo/a.jpg", false, true);
        s.set("Imágenes/Toledo/b.jpg", false, true);
        s.set("Imágenes/Otra/c.jpg", false, true);
        s.set("raiz.txt", false, true);
        let plan = s.plan();
        assert!(plan.contains(&PlanItem::Files { src_dir: "Imágenes/Toledo".into(), dest: "Toledo".into(), files: vec!["a.jpg".into(), "b.jpg".into()] }));
        assert!(plan.contains(&PlanItem::Files { src_dir: "Imágenes/Otra".into(), dest: "Otra".into(), files: vec!["c.jpg".into()] }));
        assert!(plan.contains(&PlanItem::Files { src_dir: String::new(), dest: String::new(), files: vec!["raiz.txt".into()] }));
        assert_eq!(s.check("Imágenes"), Check::Partial);
    }

    #[test]
    fn unchecking_a_file_inside_a_selected_folder_excludes_only_that_file() {
        let mut s = Selection::default();
        s.set("A", true, true);
        s.set("A/B/foto.jpg", false, false);
        assert_eq!((s.check("A/B"), s.check("A/B/foto.jpg"), s.check("A/B/otra.jpg")), (Check::Partial, Check::Off, Check::On));
        assert_eq!(s.plan(), vec![folder("A", "A", &["B/foto.jpg"])]);
    }

    #[test]
    fn rechecking_inside_an_excluded_folder_keeps_the_original_layout() {
        let mut s = Selection::default();
        s.set("A", true, true);
        s.set("A/B", true, false);
        s.set("A/B/C", true, true);
        s.set("A/B/f.txt", false, true);
        assert_eq!((s.check("A/B"), s.check("A/B/C"), s.check("A/B/D")), (Check::Partial, Check::On, Check::Off));
        let plan = s.plan();
        assert!(plan.contains(&folder("A", "A", &["B"])));
        assert!(plan.contains(&folder("A/B/C", "A/B/C", &[])));
        assert!(plan.contains(&PlanItem::Files { src_dir: "A/B".into(), dest: "A/B".into(), files: vec!["f.txt".into()] }));
    }

    #[test]
    fn nested_exclusions_apply_to_the_nearest_root() {
        let mut s = Selection::default();
        s.set("A", true, true);
        s.set("A/B", true, false);
        s.set("A/B/C", true, true);
        s.set("A/B/C/D", true, false);
        let plan = s.plan();
        assert!(plan.contains(&folder("A/B/C", "A/B/C", &["D"])));
        assert!(plan.contains(&folder("A", "A", &["B", "B/C/D"])));
    }
}
