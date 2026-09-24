# Best Practices and Advanced Topics

Professional Briev development guidelines.

## Performance Tips

### 1. Use StringBuilder for String Concatenation

```briev
// ❌ BAD - O(n²) performance
let mut s = "";
let i: Int = 0;
[i < 1000] {
    s = s + "item" + String(i);  // Creates new string each time
    i = i + 1;
};

// ✅ GOOD - O(n) performance
let mut sb = new_builder();
let i: Int = 0;
[i < 1000] {
    sb = sb.append_str("item");
    sb = sb.append_int(i);
    i = i + 1;
};
let s = sb.to_string();
```

### 2. Choose the Right Collection

| Use Case | Best Choice | Why |
|----------|-------------|-----|
| Key-value lookup | `HashMap<K,V>` | O(1) lookup |
| Membership testing | `HashSet<T>` | O(1) contains |
| LIFO processing | `Stack<T>` | O(1) push/pop |
| FIFO processing | `Queue<T>` | O(1) enqueue/dequeue |
| Ordered iteration | `List<T>` | Maintains order |
| Unique items | `HashSet<T>` | Automatic deduplication |

### 3. Minimize State Mutations

```briev
// ❌ BAD - many small mutations
txn process() {
    x = x + 1;
    y = y + 2;
    z = z + 3;
    total = x + y + z;
    term;
};

// ✅ GOOD - batch mutations
txn process() {
    let new_x = x + 1;
    let new_y = y + 2;
    let new_z = z + 3;
    let new_total = new_x + new_y + new_z;
    x = new_x;
    y = new_y;
    z = new_z;
    total = new_total;
    term;
};
```

### 4. Use Reactive Transactions Wisely

```briev
// ❌ BAD - fires too frequently
node log_everything() [true][true] {
    println("State changed");
    term;
};

// ✅ GOOD - fires only when needed
node log_important_changes() 
    [critical_value > threshold]
    [logged == true]
{
    println("Critical: " + String(critical_value));
    logged = true;
    term;
};
```

## Code Organization

### 1. Group Related Transactions

```briev
// User management
txn create_user(...) { ... }
txn update_user(...) { ... }
txn delete_user(...) { ... }
txn get_user(...) { ... }

// Authentication
txn login(...) { ... }
txn logout(...) { ... }
txn refresh_token(...) { ... }

// Authorization
txn check_permission(...) { ... }
txn grant_role(...) { ... }
txn revoke_role(...) { ... }
```

### 2. Use Modules for Large Projects

```briev
// user_module.bv
export txn create_user(...) { ... }
export txn update_user(...) { ... }
export defn get_user(...) -> User { ... }

// auth_module.bv
import "user_module";
export txn login(...) { ... }
export txn logout(...) { ... }

// main.bv
import "user_module";
import "auth_module";
```

### 3. Separate Concerns

```briev
// models.bv - Data structures
struct User { ... }
struct Product { ... }
struct Order { ... }

// services.bv - Business logic
txn create_order(...) { ... }
txn process_payment(...) { ... }

// repositories.bv - Data access
txn save_user(...) { ... }
txn find_product(...) { ... }
```

## Testing Strategies

### 1. Test Contracts

```briev
// Test that postconditions hold
defn test_increment() -> Bool {
    let old_counter = counter;
    increment();
    term counter == old_counter + 1;
};

// Test that preconditions prevent invalid states
defn test_withdraw_insufficient_funds() -> Bool {
    let old_balance = balance;
    withdraw(balance + 1);  // Should fail
    term balance == old_balance;  // Balance unchanged
};
```

### 2. Test Edge Cases

```briev
defn test_empty_list() -> Bool {
    let list: List<Int> = [];
    term list .^Len == 0;
};

defn test_zero_division() -> Bool {
    let result = safe_divide(10, 0);
    term result.is_err();
};

defn test_negative_numbers() -> Bool {
    let result = sqrt(-1.0);
    term result.is_err();
};
```

### 3. Test Reactive Chains

```briev
defn test_reactive_chain() -> Bool {
    // Set up initial state
    counter = 0;
    done = false;
    
    // Let reactive transactions fire
    run_reactor();
    
    // Verify final state
    term counter == 10 && done == true;
};
```

## Debugging Techniques

### 1. Add Logging Transactions

```briev
node log_state() [counter >= 0 && balance >= 0][counter >= 0 && balance >= 0] {
    println("Counter: " + String(counter));
    println("Balance: " + String(balance));
    println("Active: " + String(active));
    term;
};
```

### 2. Use Invariants

```briev
node check_invariants [[counter >= 0 && balance >= 0] {
    [counter >= 0] {
        // Invariant holds
    };
    [counter < 0] {
        rollback;  // Invariant violated!
    };
    
    [balance >= 0] {
        // Invariant holds
    };
    [balance < 0] {
        rollback;  // Invariant violated!
    };
    
    term;
};
```

### 3. Trace Execution Paths

```briev
let execution_log: List<String> = [];

txn log_execution(step: String) {
    execution_log = execution_log.append(step);
    term;
};

// Usage
txn process() {
    log_execution("start");
    // ... processing ...
    log_execution("complete");
    term;
};
```

## Security Best Practices

### 1. Validate All Inputs

```briev
txn create_user(username: String, email: String, password: String)
    [username .^Len >= 3 && username .^Len <= 20]
    [email.contains("@") && email.contains(".")]
    [password .^Len >= 8]
    [user_created == true]
{
    // All inputs validated by preconditions
    create_user_impl(username, email, password);
    term;
};
```

### 2. Use Access Control

```briev
let current_user: Option<User> = None;
let permissions: HashMap<String, List<String>> = new_map();

txn login(user: User, password: String) 
    [verify_password(user, password)]
    [current_user == Some(user)]
{
    current_user = Some(user);
    term;
};

txn check_permission(resource: String, action: String) 
    [current_user.is_some()]
    [permissions.get(current_user.unwrap().id).contains(resource + ":" + action)]
    [permission_granted == true]
{
    permission_granted = true;
    term;
};
```

### 3. Sanitize Outputs

```briev
defn sanitize_html(input: String) -> String {
    let mut sb = new_builder();
    let i: Int = 0;
    [i < input .^Len] {
        let c = input.char_at(i);
        [c == '<'] {
            sb = sb.append_str("&lt;");
        };
        [c == '>'] {
            sb = sb.append_str("&gt;");
        };
        [c == '&'] {
            sb = sb.append_str("&amp;");
        };
        [c != '<' && c != '>' && c != '&'] {
            sb = sb.append_char(c);
        };
        i = i + 1;
    };
    term sb.to_string();
};
```

## Deployment Checklist

- [ ] All contracts verified
- [ ] All error cases handled
- [ ] Logging enabled for debugging
- [ ] Performance benchmarks run
- [ ] Security audit completed
- [ ] Backup strategy in place
- [ ] Rollback plan documented
- [ ] Monitoring configured

## Common Pitfalls

### 1. Infinite Reactive Loops

```briev
// ❌ BAD - infinite loop
node bad_increment() [true][counter == counter + 1] {
    counter = counter + 1;
    term;
};
// Compiler will reject: cannot prove termination

// ✅ GOOD - bounded loop
node good_increment() [counter < 100][counter == counter + 1] {
    counter = counter + 1;
    term;
};
```

### 2. Race Conditions in Async Transactions

```briev
// ❌ BAD - potential race condition
async node bad_transfer() [balance >= 100][balance == balance - 100] {
    balance = balance - 100;
    term;
};

// ✅ GOOD - compiler verifies mutual exclusion
async node good_transfer() 
    [balance >= 100 && !transfer_in_progress]
    [balance == balance - 100]
{
    transfer_in_progress = true;
    balance = balance - 100;
    transfer_in_progress = false;
    term;
};
```

### 3. Ignoring Error Results

```briev
// ❌ BAD - ignores error
let result = read_file(path);
let content = result.value;  // Panics!

// ✅ GOOD - handles error
let result = read_file(path);
[result.is_ok()] {
    let content = result.value;
    process(content);
};
[result.is_err()] {
    println("Error: " + result.error.message);
};
```

## Performance Checklist

- [ ] Use `StringBuilder` for string concatenation
- [ ] Choose appropriate collections (HashMap vs List)
- [ ] Minimize state mutations
- [ ] Use reactive transactions for automatic updates
- [ ] Profile hot paths
- [ ] Cache expensive computations
- [ ] Use Metropolitan FFI for foreign calls
- [ ] Batch database operations

---

## Conclusion

You've completed the Briev tutorial! 🎉

**Next Steps:**
1. Build a complete project using Briev
2. Contribute to the Briev ecosystem
3. Share your knowledge with others
4. Explore advanced topics in the specification

**Resources:**
- [SPEC.md](../spec/SPEC.md) - Complete language specification
- [QUICK-REFERENCE.md](../spec/QUICK-REFERENCE.md) - Syntax cheat sheet
- [METROPOLITAN_FFI.md](../METROPOLITAN_FFI.md) - FFI guide
- [DATABRIEV_GUIDE.md](../DATABRIEV_GUIDE.md) - Configuration guide
- [OPTIMIZATIONS.md](../OPTIMIZATIONS.md) - Performance guide

**Get Involved:**
- Report issues on GitHub
- Contribute to the standard library
- Write tutorials and examples
- Help other learners

Happy coding! 🚀
