pub fn remove_duplicates(nums: &mut Vec<i32>) -> i32 {
    use itertools::Itertools;

    let mut tmp : Vec<i32> = nums.clone();
    tmp =  tmp.into_iter().unique().collect();
    let len = tmp.len() as i32;
    *nums = tmp;
    len
    
}

fn main() {
    let mut nums : Vec<i32> = vec![1,1,2];
    assert_eq!(remove_duplicates(&mut nums), 2);

}