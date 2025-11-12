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

pub fn add_two_numbers(l1: Option<Box<ListNode>>, l2: Option<Box<ListNode>>) -> Option<Box<ListNode>>   {
    let mut carry = 0;
    let mut result : Option<Box<ListNode>> = Some(Box::new(ListNode::new(0)));
    let mut current_result = &mut result;
    let mut current_l1 = &l1;
    let mut current_l2 = &l2;

    while let Some(n1) = current_l1 && let Some(n2) = current_l2 {
        let sum = (n1.val + n2.val + carry) % 10;
        carry = (n1.val + n2.val + carry) / 10;
        *current_result = Some(Box::new(ListNode::new(sum)));
        current_result = &mut current_result.as_mut().unwrap().next;
        current_l1 = &n1.next;
        current_l2 = &n2.next;
    }
    
    while let Some(n1) = current_l1 {
        let sum = (n1.val + carry) % 10;
        carry = (n1.val + carry) / 10;
        *current_result = Some(Box::new(ListNode::new(sum)));
        current_result = &mut current_result.as_mut().unwrap().next;
        current_l1 = &n1.next;
    }

    while let Some(n2) = current_l2 {
        let sum = (n2.val + carry) % 10;
        carry = (n2.val + carry) / 10;
        *current_result = Some(Box::new(ListNode::new(sum)));
        current_result = &mut current_result.as_mut().unwrap().next;
        current_l2 = &n2.next;
    }    
    
    if carry > 0 {
        *current_result = Some(Box::new(ListNode::new(carry)));
    }

    result
}

fn main() {
    let l1 = Some(Box::new(ListNode::new(2)));
    let l2 = Some(Box::new(ListNode::new(3)));
    assert_eq!(add_two_numbers(l1, l2), Some(Box::new(ListNode::new(5))));
}
