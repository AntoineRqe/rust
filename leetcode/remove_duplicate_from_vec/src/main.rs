pub fn remove_duplicates(nums: &mut Vec<i32>) -> i32 {
    use itertools::Itertools;

    let mut tmp : Vec<i32> = nums.clone();
    tmp =  tmp.into_iter().unique().collect();
    let len = tmp.len() as i32;
    *nums = tmp;
    len
    
}

pub fn merge(nums1: &mut Vec<i32>, m: i32, nums2: &mut Vec<i32>, n: i32) {
        if n == 0 { nums1.truncate(m as usize); return; }
        if m == 0 { nums1.clear(); nums1.append(nums2); return };
    

        let mut index : usize = 0;
        let mut nums1_index_end = m as usize;

        for tmp_num2 in nums2
        {
            println!("Comparing {} vs {}", tmp_num2, nums1[index]);
            println!("Index {} nums1 end {}", index, nums1_index_end);

            while *tmp_num2 >= nums1[index] && index < nums1_index_end {
                index += 1;
                println!("Comparing {} vs {}", tmp_num2, nums1[index]);
            }

            println!("Inserting {} n pos {}", tmp_num2, index);
            nums1.insert(index, *tmp_num2);
            nums1.pop();
            index += 1;
            nums1_index_end += 1;
            //TODO
        }
}

fn main() {
    let mut nums : Vec<i32> = vec![1,1,2];
    assert_eq!(remove_duplicates(&mut nums), 2);

}