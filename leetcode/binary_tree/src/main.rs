// Definition for a binary tree node.
#[derive(Debug, PartialEq, Eq)]
pub struct TreeNode {
  pub val: i32,
  pub left: Option<Rc<RefCell<TreeNode>>>,
  pub right: Option<Rc<RefCell<TreeNode>>>,
}

impl TreeNode {
  #[inline]
  pub fn new(val: i32) -> Self {
    TreeNode {
      val,
      left: None,
      right: None
    }
  }
}

use std::rc::Rc;
use std::cell::RefCell;
use std::collections::VecDeque;

fn count_tree_leaf(node: &Option<Rc<RefCell<TreeNode>>>) -> u32 {
    match node {
        Some(node) => { return (1 +
                             self::count_tree_leaf(&node.borrow().left) +
                             self::count_tree_leaf(&node.borrow().right)) as u32
                      },
        None => { return 0;}
    }   
}

fn create_tree_from_list(values: Vec<Option<i32>>) ->  Option<Rc<RefCell<TreeNode>>> {
    if values.len() == 0 || values[0].is_none() { return None; }

    let root = Rc::new(RefCell::new(TreeNode::new(values[0].unwrap())));
    let mut queue = VecDeque::new();

    queue.push_back(Rc::clone(&root));
    let mut index = 1;

    loop {

        if index >= values.len() { break;}

        let node = queue.pop_front().unwrap();

        if let Some(left_value) = values[index] {
            let left_node = Rc::new(RefCell::new(TreeNode::new(left_value)));
            node.borrow_mut().left = Some(Rc::clone(&left_node));
            queue.push_back(Rc::clone(&left_node));
        }
        index += 1;

        if index < values.len() && let Some(right_value) = values[index] {
            let right_node = Rc::new(RefCell::new(TreeNode::new(right_value)));
            node.borrow_mut().right = Some(Rc::clone(&right_node));
            queue.push_back(Rc::clone(&right_node));
        }
        index += 1;
    }

    Some(root)
}

pub fn inorder_traversal(root: Option<Rc<RefCell<TreeNode>>>) -> Vec<i32> {
    let mut values : Vec<i32> = vec![];

    if let Some(node) = root {
        values.append(&mut self::inorder_traversal(node.borrow().left.clone()));
        values.push(node.borrow().val);
        values.append(&mut self::inorder_traversal(node.borrow().right.clone()));
    }

    return values;
}

pub fn inorder_traversal_iterative(root: Option<Rc<RefCell<TreeNode>>>) -> Vec<i32> {
    let mut stack = vec![];
    let mut res = vec![];
    let mut current = root;

    while current.is_some() || !stack.is_empty() {
        while let Some(node) = current {
            stack.push(node.clone());
            current = node.borrow().left.clone();
        }
        
        if let Some(node) = stack.pop() {
            res.push(node.borrow().val);
            current = node.borrow().right.clone();
        }
    }
    return res;
}


pub fn is_same_tree(p: Option<Rc<RefCell<TreeNode>>>, q: Option<Rc<RefCell<TreeNode>>>) -> bool {

    use std::collections::VecDeque;

    let mut queue = VecDeque::new();

    queue.push_back((p, q));

    while let Some((p, q)) = queue.pop_front() {
        match (p, q) {
            (None, None) => {},
            (Some(p), Some(q)) =>  {
                if p.borrow().val != q.borrow().val { return false; }
                queue.push_back((p.borrow().left.clone(), q.borrow().left.clone()));
                queue.push_back((p.borrow().right.clone(), q.borrow().right.clone()));
            },
            _ => { return false; }
        }
    }

    return queue.is_empty()
}

pub fn invert_tree(root: Option<Rc<RefCell<TreeNode>>>) -> Option<Rc<RefCell<TreeNode>>> {
    let mut queue = std::collections::VecDeque::new();
    if let Some(root_node) = root.clone() {
        queue.push_back(root_node);
    }
    while let Some(node) = queue.pop_front() {
        let mut node_borrow = node.borrow_mut();
        // Swap left and right
        let left = node_borrow.left.take();
        let right = node_borrow.right.take();
    
        node_borrow.left = right;
        node_borrow.right = left;

        // Enqueue children
        if let Some(left) = &node_borrow.left {
            queue.push_back(left.clone());
        }
        if let Some(right) = &node_borrow.right {
            queue.push_back(right.clone());
        }
    }
    root
}

pub fn is_symmetric(root: Option<Rc<RefCell<TreeNode>>>) -> bool {

    let mut queue = std::collections::VecDeque::new();
    if let Some(root_node) = root {
        queue.push_back((root_node.borrow().left.clone(), root_node.borrow().right.clone()));
    }

    while let Some((left, right)) = queue.pop_front() {
        match (left, right) {
            (None, None) => continue,
            (Some(lefty), Some(righty)) => {
                if lefty.borrow().val != righty.borrow().val {
                    return false;
                }
                queue.push_back((lefty.borrow().left.clone(), righty.borrow().right.clone()));
                queue.push_back((lefty.borrow().right.clone(), righty.borrow().left.clone()));
            },
            _ => return false,
        }
    }

    queue.is_empty()
}


fn main() {
    let test = vec![Some(1),Some(2),Some(3),Some(4),Some(5),Some(6),Some(7),Some(8)];
    let root = create_tree_from_list(test);
    let mirror = vec![Some(1),Some(2),Some(2),Some(3),Some(4),Some(4),Some(3)];
    let root_mirror = create_tree_from_list(mirror);
    assert!(is_symmetric(root_mirror));    
}