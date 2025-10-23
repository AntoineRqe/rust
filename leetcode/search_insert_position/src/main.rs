pub fn search_insert(nums: Vec<i32>, target: i32) -> i32 {
        let nums_len = nums.len();
        if nums[0] >= target { return 0;}
        else if nums[nums_len - 1] < target {return nums_len as i32;}

        let mut left = 0;
        let mut right = nums_len;
    
        loop {
            let m = (left + right)/2;

            if nums[m] < target {
                left = m;
                
            } else if nums[m] > target {
                right = m;
            } else {
                return m as i32;
            }

            if right - left < 2 {
                if nums[right] < target { return left as i32;}
                else {return right as i32;}
            }
        }
}

fn main() {
    assert_eq!(search_insert(vec![1,3,5,6], 5), 2);
    assert_eq!(search_insert(vec![1,3,5,6], 2), 1);
    assert_eq!(search_insert(vec![1,3,5,6], 7), 4);
}
