// Definition for singly-linked list.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct ListNode {
  pub val: i32,
  pub next: Option<Box<ListNode>>
}

impl ListNode {
  #[inline]
  fn new(val: i32) -> Self {
    ListNode {
      next: None,
      val
    }
  }
}

fn merge_two_lists(mut list1: Option<Box<ListNode>>, mut list2: Option<Box<ListNode>>) -> Option<Box<ListNode>> {

    let mut new_head = None;
    let mut current = &mut new_head;

    while list1.is_some() && list2.is_some() {
        if list1.as_ref().unwrap().val < list2.as_ref().unwrap().val {
            let mut node = list1.take();
            let next = node.as_mut().unwrap().next.take();
            *current = node;
            list1 = next;
        } else {
            let mut node = list2.take();
            let next = node.as_mut().unwrap().next.take();
            *current = node;
            list2 = next;
        }
        current = &mut current.as_mut().unwrap().next;
    }

    *current = if list1.is_some() { list1 } else { list2 };

    new_head
}

fn create_list(items: Vec<i32>) -> Option<Box<ListNode>> {
    if items.is_empty() {
        return None;
    }

    let mut head = Some(Box::new(ListNode::new(items[0])));
    let mut current = &mut head;

    for &item in &items[1..] {
        if let Some(node) = current {
            node.next = Some(Box::new(ListNode::new(item)));
            current = &mut node.next;
        }
    }
    
    head
}

fn main() {
    let list1 = create_list(vec![1,2,4]);
    let list2 = create_list(vec![1,2,3]);
    let _ = merge_two_lists(list1, list2);
}