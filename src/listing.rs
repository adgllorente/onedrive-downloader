use std::cmp::Ordering;

use crate::text;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: String,
    pub name: String,
    pub folded: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub modified: Option<i64>,
}

impl Entry {
    pub fn new(parent: &str, name: &str, is_dir: bool, size: Option<u64>, modified: Option<i64>) -> Self {
        Self {
            path: if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") },
            name: name.to_string(),
            folded: text::fold(name),
            is_dir,
            size,
            modified,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexItem {
    pub path: String,
    pub name: String,
    pub folded_name: String,
    pub folded_path: String,
    pub modified: Option<i64>,
}

impl IndexItem {
    pub fn new(path: &str, modified: Option<i64>) -> Self {
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        Self {
            folded_name: text::fold(&name),
            folded_path: text::fold(path),
            name,
            path: path.to_string(),
            modified,
        }
    }

    pub fn parent(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(parent, _)| parent)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    NameAsc,
    NameDesc,
    DateDesc,
    DateAsc,
    SizeDesc,
    SizeAsc,
}

impl SortKey {
    pub const ALL: [SortKey; 6] = [
        SortKey::NameAsc,
        SortKey::NameDesc,
        SortKey::DateDesc,
        SortKey::DateAsc,
        SortKey::SizeDesc,
        SortKey::SizeAsc,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::NameAsc => "Nombre (A-Z)",
            SortKey::NameDesc => "Nombre (Z-A)",
            SortKey::DateDesc => "Fecha (más recientes primero)",
            SortKey::DateAsc => "Fecha (más antiguas primero)",
            SortKey::SizeDesc => "Tamaño (mayores primero)",
            SortKey::SizeAsc => "Tamaño (menores primero)",
        }
    }
}

fn by_key(key: SortKey, a: (&str, Option<i64>, Option<u64>), b: (&str, Option<i64>, Option<u64>)) -> Ordering {
    match key {
        SortKey::NameAsc => text::natural_cmp(a.0, b.0),
        SortKey::NameDesc => text::natural_cmp(b.0, a.0),
        SortKey::DateDesc => b.1.unwrap_or(i64::MIN).cmp(&a.1.unwrap_or(i64::MIN)),
        SortKey::DateAsc => a.1.unwrap_or(i64::MAX).cmp(&b.1.unwrap_or(i64::MAX)),
        SortKey::SizeDesc => b.2.unwrap_or(0).cmp(&a.2.unwrap_or(0)),
        SortKey::SizeAsc => a.2.unwrap_or(u64::MAX).cmp(&b.2.unwrap_or(u64::MAX)),
    }
}

pub fn compare_entries(key: SortKey, a: &Entry, b: &Entry) -> Ordering {
    b.is_dir
        .cmp(&a.is_dir)
        .then_with(|| by_key(key, (&a.folded, a.modified, a.size), (&b.folded, b.modified, b.size)))
        .then_with(|| text::natural_cmp(&a.folded, &b.folded))
}

pub fn compare_index(key: SortKey, a: &IndexItem, b: &IndexItem) -> Ordering {
    by_key(key, (&a.folded_name, a.modified, None), (&b.folded_name, b.modified, None))
        .then_with(|| text::natural_cmp(&a.folded_path, &b.folded_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str, modified: i64) -> Entry {
        Entry::new("", name, true, None, Some(modified))
    }

    fn file(name: &str, size: u64, modified: i64) -> Entry {
        Entry::new("", name, false, Some(size), Some(modified))
    }

    fn sorted(key: SortKey, mut entries: Vec<Entry>) -> Vec<String> {
        entries.sort_by(|a, b| compare_entries(key, a, b));
        entries.into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn entry_path_joins_parent_and_name() {
        assert_eq!(Entry::new("", "Imágenes", true, None, None).path, "Imágenes");
        assert_eq!(Entry::new("Imágenes/2015", "a.jpg", false, Some(1), None).path, "Imágenes/2015/a.jpg");
    }

    #[test]
    fn folders_come_first_and_names_sort_naturally_ignoring_accents() {
        let entries = vec![file("b.jpg", 1, 0), dir("Zeta", 0), dir("álbum 10", 0), dir("Álbum 2", 0)];
        assert_eq!(sorted(SortKey::NameAsc, entries.clone()), vec!["Álbum 2", "álbum 10", "Zeta", "b.jpg"]);
        assert_eq!(sorted(SortKey::NameDesc, entries), vec!["Zeta", "álbum 10", "Álbum 2", "b.jpg"]);
    }

    #[test]
    fn date_sort_puts_unknown_dates_last() {
        let mut undated = dir("sin fecha", 0);
        undated.modified = None;
        let entries = vec![dir("viejo", 10), undated, dir("nuevo", 99)];
        assert_eq!(sorted(SortKey::DateDesc, entries.clone()), vec!["nuevo", "viejo", "sin fecha"]);
        assert_eq!(sorted(SortKey::DateAsc, entries), vec!["viejo", "nuevo", "sin fecha"]);
    }

    #[test]
    fn size_sort_orders_files_and_leaves_folders_by_name() {
        let entries = vec![file("medio", 50, 0), dir("b", 0), file("grande", 900, 0), dir("a", 0), file("pequeño", 5, 0)];
        assert_eq!(sorted(SortKey::SizeDesc, entries.clone()), vec!["a", "b", "grande", "medio", "pequeño"]);
        assert_eq!(sorted(SortKey::SizeAsc, entries), vec!["a", "b", "pequeño", "medio", "grande"]);
    }

    #[test]
    fn index_item_exposes_name_and_parent() {
        let item = IndexItem::new("Imágenes/2015/Toledo RAW", Some(5));
        assert_eq!((item.name.as_str(), item.parent()), ("Toledo RAW", "Imágenes/2015"));
        assert_eq!(item.folded_path, "imagenes/2015/toledo raw");
        assert_eq!(IndexItem::new("Raiz", None).parent(), "");
    }

    #[test]
    fn index_sorts_by_name_or_date() {
        let mut items = [IndexItem::new("x/b", Some(1)), IndexItem::new("x/a", Some(2)), IndexItem::new("y/c", None)];
        items.sort_by(|a, b| compare_index(SortKey::NameAsc, a, b));
        assert_eq!(items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        items.sort_by(|a, b| compare_index(SortKey::DateDesc, a, b));
        assert_eq!(items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    }
}
