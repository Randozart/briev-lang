# Common Patterns and Best Practices

 idiomatic Briev code and proven design patterns.

## 1. State Machine Pattern

Use enums and reactive transactions for state machines:

```briev
enum OrderState { Pending, Paid, Shipped, Delivered, Cancelled }
let state: OrderState = OrderState::Pending;

node mark_paid() 
    [state == OrderState::Pending]
    [state == OrderState::Paid]
{
    state = OrderState::Paid;
    term;
};

node mark_shipped() 
    [state == OrderState::Paid]
    [state == OrderState::Shipped]
{
    state = OrderState::Shipped;
    term;
};

node mark_delivered() 
    [state == OrderState::Shipped]
    [state == OrderState::Delivered]
{
    state = OrderState::Delivered;
    term;
};

node cancel() 
    [state == OrderState::Pending || state == OrderState::Paid]
    [state == OrderState::Cancelled]
{
    state = OrderState::Cancelled;
    term;
};
```

**Benefits:**
- ✅ Compiler verifies all transitions
- ✅ No invalid states possible
- ✅ Self-documenting state flow

## 2. Observer Pattern

Use reactive transactions for automatic notifications:

```briev
let observers: List<String> = [];
let subject_value: Int = 0;
let notified_value: Int = -1;

node notify_observers() 
    [subject_value != notified_value]
    [notified_value == subject_value]
{
    let i: Int = 0;
    [i < observers ^^Len] {
        send_notification(observers[i], subject_value);
        i = i + 1;
    };
    notified_value = subject_value;
    term;
};

txn subscribe(observer: String) [true][observers.contains(observer)] {
    observers = observers.append(observer);
    term;
};
```

## 3. CQRS (Command Query Responsibility Segregation)

Separate state-changing transactions from queries:

```briev
// Commands (state-changing)
txn create_user(name: String, email: String) 
    [!user_exists(email)]
    [users ^^Len == users ^^Len + 1]
{
    let user = User { id: next_id(), name: name, email: email };
    users = users.append(user);
    term;
};

txn update_user(id: Int, name: String) 
    [user_exists_by_id(id)]
    [true]
{
    let i: Int = 0;
    [i < users ^^Len] {
        [users[i].id == id] {
            users[i].name = name;
        };
        i = i + 1;
    };
    term;
};

// Queries (read-only, no state mutation)
defn get_user(id: Int) -> Option<User> {
    let i: Int = 0;
    [i < users ^^Len] {
        [users[i].id == id] {
            term Some(users[i]);
        };
        i = i + 1;
    };
    term None;
};

defn list_users() -> List<User> {
    term users;
};
```

## 4. Repository Pattern

Encapsulate data access logic:

```briev
struct UserRepository {
    cache: HashMap<Int, User>;
    dirty: Bool;
};

// Transactions on a struct use the struct name as prefix
txn get(repo: Ptr<UserRepository>, id: Int) -> Option<User> {
    [repo.cache.contains_key(id)] {
        term repo.cache.get(id);
    };
    let user = db_find_user(id);
    repo.cache = repo.cache.insert(id, user);
    term user;
};

txn save(repo: Ptr<UserRepository>, user: User) [true][repo.cache.contains_key(user.id)] {
    repo.cache = repo.cache.insert(user.id, user);
    repo.dirty = true;
    term;
};

txn flush(repo: Ptr<UserRepository>) [repo.dirty][!repo.dirty] {
    db_save_all(repo.cache.values());
    repo.dirty = false;
    term;
};
```

## 5. Builder Pattern

Construct complex objects step-by-step:

```briev
struct RequestBuilder {
    url: String,
    method: String,
    headers: HashMap<String, String>,
    body: Option<String>
};

defn new_request_builder(url: String) -> RequestBuilder {
    term RequestBuilder {
        url: url,
        method: "GET",
        headers: new_map(),
        body: None
    };
};

defn builder_method(builder: RequestBuilder, method: String) -> RequestBuilder {
    term RequestBuilder {
        url: builder.url,
        method: method,
        headers: builder.headers,
        body: builder.body
    };
};

defn builder_header(builder: RequestBuilder, key: String, value: String) -> RequestBuilder {
    let headers = builder.headers.insert(key, value);
    term RequestBuilder {
        url: builder.url,
        method: builder.method,
        headers: headers,
        body: builder.body
    };
};

defn builder_body(builder: RequestBuilder, body: String) -> RequestBuilder {
    term RequestBuilder {
        url: builder.url,
        method: builder.method,
        headers: builder.headers,
        body: Some(body)
    };
};

defn builder_build(builder: RequestBuilder) -> Request {
    term Request {
        url: builder.url,
        method: builder.method,
        headers: builder.headers,
        body: builder.body
    };
};

// Usage
let request = builder_build(
    builder_body(
        builder_header(
            builder_method(
                new_request_builder("https://api.example.com"),
                "POST"
            ),
            "Content-Type",
            "application/json"
        ),
        "{\"key\": \"value\"}"
    )
);
```

## 6. Strategy Pattern

Swap algorithms at runtime:

```briev
enum SortStrategy { BubbleSort, QuickSort, MergeSort }
let strategy: SortStrategy = SortStrategy::QuickSort;

txn sort(list: List<Int>) -> List<Int> {
    when strategy == SortStrategy::BubbleSort {
        term bubble_sort(list);
    };
    when strategy == SortStrategy::QuickSort {
        term quick_sort(list);
    };
    when strategy == SortStrategy::MergeSort {
        term merge_sort(list);
    };
    term list;
};

txn set_strategy(new_strategy: SortStrategy) [true][strategy == new_strategy] {
    strategy = new_strategy;
    term;
};
```

## 7. Circuit Breaker Pattern

Prevent cascade failures:

```briev
enum CircuitState { Closed, Open, HalfOpen }
let circuit_state: CircuitState = CircuitState::Closed;
let failure_count: Int = 0;
let last_failure_time: Int = 0;

node trip_circuit() 
    [failure_count >= 5 && circuit_state == CircuitState::Closed]
    [circuit_state == CircuitState::Open]
{
    circuit_state = CircuitState::Open;
    last_failure_time = current_time();
    term;
};

node reset_circuit() 
    [circuit_state == CircuitState::HalfOpen && success_count >= 3]
    [circuit_state == CircuitState::Closed && failure_count == 0]
{
    circuit_state = CircuitState::Closed;
    failure_count = 0;
    term;
};

node test_circuit() 
    [circuit_state == CircuitState::Open && current_time() - last_failure_time > 60000]
    [circuit_state == CircuitState::HalfOpen]
{
    circuit_state = CircuitState::HalfOpen;
    term;
};

defn call_external_service() -> Result<String, String> {
    [circuit_state == CircuitState::Open] {
        term Err("Circuit is open");
    };
    
    let result = external_call();
    [result.is_ok()] {
        failure_count = 0;
    };
    [result.is_err()] {
        failure_count = failure_count + 1;
    };
    
    term result;
};
```

## 8. Retry with Backoff

```briev
let attempts: Int = 0;
let last_attempt_time: Int = 0;

node retry_operation() 
    [!operation_success && attempts < 5]
    [attempts == attempts + 1]
{
    let delay = math.pow(2, attempts) * 1000;  // Exponential backoff
    [current_time() - last_attempt_time >= delay] {
        let result = try_operation();
        [result.is_ok()] {
            operation_success = true;
        };
        [result.is_err()] {
            attempts = attempts + 1;
            last_attempt_time = current_time();
        };
    };
    term;
};
```

## 9. Rate Limiting

```briev
let request_count: Int = 0;
let window_start: Int = 0;

node reset_window() 
    [current_time() - window_start >= 60000]  // 1 minute
    [request_count == 0 && window_start == current_time()]
{
    request_count = 0;
    window_start = current_time();
    term;
};

txn process_request() 
    [request_count < 100]
    [request_count == request_count + 1]
{
    request_count = request_count + 1;
    handle_request();
    term;
};
```

## 10. Caching Pattern

```briev
let cache: HashMap<String, CacheEntry> = new_map();
let cache_size: Int = 100;

struct CacheEntry {
    value: String,
    timestamp: Int,
    ttl: Int
};

defn get_cached(key: String) -> Option<String> {
    [cache.contains_key(key)] {
        let entry = cache.get(key).unwrap();
        [current_time() - entry.timestamp < entry.ttl] {
            term Some(entry.value);
        };
    };
    term None;
};

txn cache_put(key: String, value: String, ttl: Int) 
    [true]
    [cache.contains_key(key)]
{
    let entry = CacheEntry {
        value: value,
        timestamp: current_time(),
        ttl: ttl
    };
    
    [cache ^^Len >= cache_size] {
        cache = evict_oldest(cache);
    };
    
    cache = cache.insert(key, entry);
    term;
};
```

## Best Practices

### 1. Write Meaningful Contracts

```briev
// ❌ BAD - provides no information
txn process() [true][true] { ... }

// ✅ GOOD - documents requirements and guarantees
txn withdraw(amount: Int) 
    [amount > 0 && balance >= amount]  // When it can run
    [balance == balance - amount]      // What it guarantees
{ ... };
```

### 2. Keep Transactions Small

```briev
// ❌ BAD - too much logic in one transaction
txn process_order() {
    validate_order();
    calculate_total();
    charge_payment();
    update_inventory();
    send_confirmation();
    term;
};

// ✅ GOOD - separate concerns
txn validate_order() [order_valid][validated == true] { ... }
txn calculate_total() [validated][total_calculated == true] { ... }
txn charge_payment() [total_calculated][payment_charged == true] { ... }
txn update_inventory() [payment_charged][inventory_updated == true] { ... }
txn send_confirmation() [inventory_updated][sent == true] { ... }
```

### 3. Use Reactive Transactions for Side Effects

```briev
// ❌ BAD - manual polling
node check_email() [true][true] {
    [has_new_email()] {
        process_email();
    };
    term;
};

// ✅ GOOD - reactive
node process_new_email() [has_new_email()][email_processed == true] {
    process_email();
    term;
};
```

### 4. Handle All Error Cases

```briev
// ❌ BAD - ignores errors
let result = read_file(path);
let content = result.value;  // Panics if error!

// ✅ GOOD - handles errors
let result = read_file(path);
[result.is_ok()] {
    let content = result.value;
    process(content);
};
[result.is_err()] {
    println("Error: " + result.error.message);
};
```

### 5. Document with Contracts

```briev
// Self-documenting code
defn binary_search(list: List<Int>, target: Int) 
    [is_sorted(list)]              // Requires sorted list
    [result >= -1]                  // Returns index or -1
    -> Int 
{
    [result >= 0] {
        [list[result] == target]    // If found, it matches
    };
    term search_impl(list, target, 0, list ^^Len - 1);
};
```

---

*Next: [10-best-practices.md](10-best-practices.md) - Advanced topics and performance tips*
