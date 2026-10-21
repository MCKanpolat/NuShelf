use std::collections::{HashMap, HashSet, VecDeque};

/// A single shortest declared dependency path. Cycles never recurse.
pub(super) fn parents(root: Option<&str>, edges: &[(String, String)]) -> HashMap<String, String> {
    let Some(root) = root else {
        return HashMap::new();
    };
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for (parent, child) in edges {
        children.entry(parent).or_default().push(child);
    }
    let mut seen = HashSet::from([root]);
    let mut queue = VecDeque::from([root]);
    let mut parents = HashMap::new();
    while let Some(parent) = queue.pop_front() {
        if let Some(nodes) = children.get(parent) {
            for child in nodes {
                if seen.insert(child) {
                    parents.insert((*child).to_owned(), parent.to_owned());
                    queue.push_back(child);
                }
            }
        }
    }
    parents
}
pub(super) fn path(
    root: Option<&str>,
    reference: &str,
    parents: &HashMap<String, String>,
) -> Option<Vec<String>> {
    let root = root?;
    let mut current = reference;
    let mut result = vec![reference.to_owned()];
    while current != root {
        current = parents.get(current)?;
        result.push(current.to_owned());
    }
    result.reverse();
    Some(result)
}
