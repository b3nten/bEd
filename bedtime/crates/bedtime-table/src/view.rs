//! Which rows show, in what order: a filter (rows with a cell containing
//! it, ignoring case) and a sort by one column (numbers as numbers). Neither
//! changes the file.
use crate::Table;
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub column: usize,
    pub descending: bool,
}

/// The rows shown, as indices into the table (after the header, if it has one).
pub fn shown_rows(table: &Table, header: bool, filter: &str, sort: Option<Sort>) -> Vec<usize> {
    let first = usize::from(header && !table.rows().is_empty());
    let needle = filter.trim().to_lowercase();
    let mut rows: Vec<usize> = (first..table.rows().len())
        .filter(|&row| {
            needle.is_empty()
                || table.rows()[row]
                    .iter()
                    .any(|cell| cell.to_lowercase().contains(&needle))
        })
        .collect();
    if let Some(sort) = sort {
        rows.sort_by(|&a, &b| {
            let (a, b) = (table.cell(a, sort.column), table.cell(b, sort.column));
            let order = compare(a, b);
            // Empty cells stay last either way.
            let empty = a.trim().is_empty() || b.trim().is_empty();
            if sort.descending && !empty {
                order.reverse()
            } else {
                order
            }
        });
    }
    rows
}

/// Numbers by value, before text; text naturally ("a2" before "a10"),
/// ignoring case; empty cells last.
pub fn compare(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.trim(), b.trim());
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    let number = |s: &str| {
        s.replace(',', "")
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
    };
    match (number(a), number(b)) {
        (Some(x), Some(y)) => return x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        (Some(_), None) => return Ordering::Less,
        (None, Some(_)) => return Ordering::Greater,
        _ => {}
    }
    natural(&a.to_lowercase(), &b.to_lowercase())
}

fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut digits = String::new();
                    while let Some(c) = chars.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        chars.next();
                    }
                    digits
                };
                let (x, y) = (take(&mut a), take(&mut b));
                let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                a.next();
                b.next();
            }
        }
    }
}

/// A guess: the first row is a header if there are more, and its cells are
/// different and not numbers.
pub fn looks_like_header(table: &Table) -> bool {
    let rows = table.rows();
    let Some(first) = rows.first() else {
        return false;
    };
    let numeric = |row: &Vec<String>| row.iter().any(|cell| cell.trim().parse::<f64>().is_ok());
    let unique = {
        let mut cells: Vec<&String> = first.iter().filter(|c| !c.is_empty()).collect();
        let len = cells.len();
        cells.sort();
        cells.dedup();
        cells.len() == len
    };
    unique && !numeric(first) && rows.len() > 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_filesystem::FileModel;

    #[test]
    fn filters_and_sorts_without_the_header() {
        let table = Table::decode(b"name,size\nb10,10\na2,2\nB9,\nc,100\n").unwrap();
        assert!(looks_like_header(&table));
        assert_eq!(shown_rows(&table, true, "", None), [1, 2, 3, 4]);
        let by_name = Some(Sort {
            column: 0,
            descending: false,
        });
        assert_eq!(shown_rows(&table, true, "", by_name), [2, 3, 1, 4]);
        let by_size = Some(Sort {
            column: 1,
            descending: true,
        });
        assert_eq!(shown_rows(&table, true, "", by_size), [4, 1, 2, 3]);
        assert_eq!(shown_rows(&table, true, "B", None), [1, 3]);
        assert_eq!(shown_rows(&table, false, "", None).len(), 5);
    }

    #[test]
    fn natural_order() {
        assert_eq!(compare("a2", "a10"), Ordering::Less);
        assert_eq!(compare("1,000", "999"), Ordering::Greater);
        assert_eq!(compare("", "z"), Ordering::Greater);
        assert_eq!(compare("Apple", "apple"), Ordering::Equal);
    }
}
