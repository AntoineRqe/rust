fn two_sum_brute_force(nums: Vec<i32>, target: i32) -> Vec<i32> {
    for i in 0 .. nums.len() {
        for j in i + 1 .. nums.len() {
            if nums[i] + nums[j] == target {
                return vec![i as i32,j as i32];
            }
        }
    }
    vec![]
}

fn two_sum_hashmap(nums: Vec<i32>, target: i32) -> Vec<i32> {
    use std::collections::HashMap;

    let mut tmp = HashMap::new();
    for (i, val) in nums.iter().enumerate() {
        tmp.insert(val, i);
    }

    for i in 0 .. nums.len() {
        let complement = target - nums[i];
        if tmp.contains_key(&complement) {
            let index = *tmp.get(&complement).unwrap();
            if index == i { continue };
            return vec![i as i32, *tmp.get(&complement).unwrap() as i32];
        }
    }
    vec![]
}

fn main() {
    let nums = vec![2,7,11,15];
    let target = 9;

    let res = two_sum_brute_force(nums.clone(), target);
    let res = two_sum_hashmap(nums.clone(), target);
    println!("{res:?}");
}
