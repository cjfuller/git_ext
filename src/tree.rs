use std::cell::RefCell;
use std::iter::{once, repeat_n};
use std::rc::Rc;

pub struct TC;

impl TC {
    pub const VERTICAL: char = '│';
    pub const VERTICAL_T: char = '├';
    pub const HORIZONTAL_T: char = '┬';
    pub const CURVED_CORNER: char = '╰';
    pub const INTERSECTION: char = '┼';
    pub const HORIZONTAL: char = '─';
}

pub enum Tree<T> {
    Leaf(T),
    Node {
        value: T,
        children: Vec<Rc<RefCell<Tree<T>>>>,
    },
}

impl<T> Clone for Tree<T>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        match self {
            Self::Leaf(arg0) => Self::Leaf(arg0.clone()),
            Self::Node { value, children } => Self::Node {
                value: value.clone(),
                children: children.clone(),
            },
        }
    }
}

impl<T> Tree<T> {
    pub fn value(&self) -> &T {
        match self {
            Tree::Leaf(t) => t,
            Tree::Node { value: t, .. } => t,
        }
    }
    pub fn children(&self) -> Option<&[Rc<RefCell<Tree<T>>>]> {
        match self {
            Tree::Leaf(_) => None,
            Tree::Node { children, .. } => Some(children),
        }
    }
}

pub fn draw_from<T: std::fmt::Display + Clone>(
    root: &Tree<T>,
    offset: usize,
    first_node_prefix: String,
    child_prefix: String,
) -> Vec<(T, String)> {
    let offset = std::cmp::max(offset, 1);
    let mut output = vec![];
    output.push((
        root.value().clone(),
        format!("{first_node_prefix} {}", root.value()),
    ));

    for (i, child) in root.children().unwrap_or_default().iter().enumerate() {
        let mut next_node_prefix = format!(
            "{child_prefix}{}",
            &repeat_n(' ', offset - 1).collect::<String>()
        );

        let next_child_prefix: String = if i == root.children().unwrap_or_default().len() - 1 {
            next_node_prefix.push(TC::CURVED_CORNER);
            child_prefix.chars().chain(repeat_n(' ', offset)).collect()
        } else {
            next_node_prefix.push(TC::VERTICAL_T);
            child_prefix
                .chars()
                .chain(repeat_n(' ', offset - 1))
                .chain(once(TC::VERTICAL))
                .collect()
        };

        output.append(&mut draw_from(
            &*child.borrow(),
            offset,
            next_node_prefix,
            next_child_prefix,
        ));
    }

    output
}
