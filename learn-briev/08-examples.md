# Complete Examples

Real-world Briev programs demonstrating all features.

## 1. Counter Application

```briev
// counter.rbv
let count: Int = 0;

txn increment [count < 100][count + 1 == count] {
    count = count + 1;
    term;
};

txn decrement [count > 0][count - 1 == count] {
    count = count - 1;
    term;
};

txn reset [count != 0][0 == count] {
    count = 0;
    term;
};

render struct Counter {
    <div class="counter">
        <h1 b-text="count"></h1>
        <button b-trigger:click="increment">+</button>
        <button b-trigger:click="decrement">-</button>
        <button b-trigger:click="reset">Reset</button>
    </div>
};
```

See `examples/counter.rbv` for the full file with `<view>` and `<style>` blocks.
State variables are declared with `let` at the top level; `render struct` attaches
the HTML template that binds to those variables via `b-*` directives.

## 2. Bank Account System

```briev
// bank.bv
let balance: Int = 1000;
let overdraft_protection: Bool = false;

txn deposit(amount: Int) 
    [amount > 0]
    [balance == balance + amount]
{
    balance = balance + amount;
    term;
};

txn withdraw(amount: Int) 
    [amount > 0 && (balance >= amount || overdraft_protection)]
    [balance == balance - amount]
{
    balance = balance - amount;
    term;
};

txn transfer(to_account: Int, amount: Int)
    [amount > 0 && balance >= amount]
    [balance == balance - amount]
{
    balance = balance - amount;
    send_to(to_account, amount);
    term;
};

node apply_interest() 
    [balance > 10000]
    [balance == balance + (balance * 5 / 100)]
{
    let interest = balance * 5 / 100;
    balance = balance + interest;
    term;
};

txn enable_overdraft() 
    [!overdraft_protection]
    [overdraft_protection == true]
{
    overdraft_protection = true;
    term;
};
```

## 3. Shopping Cart

```briev
// shopping_cart.rbv
let items: Int = 0;
let total: Float = 0.0;
let discount_applied: Bool = false;

txn add_item(price: Float, quantity: Int)
    [price > 0 && quantity > 0]
    [items == items + quantity && total == total + (price * quantity)]
{
    items = items + quantity;
    total = total + (price * quantity);
    term;
};

txn remove_item(quantity: Int)
    [quantity > 0 && quantity <= items]
    [items == items - quantity]
{
    items = items - quantity;
    term;
};

txn clear_cart [items > 0][items == 0 && total == 0.0] {
    items = 0;
    total = 0.0;
    discount_applied = false;
    term;
};

node apply_bulk_discount 
    [items > 10 && total > 100.0 && !discount_applied]
    [total < total && discount_applied == true]
{
    let discount: Float = total * 0.1;
    total = total - discount;
    discount_applied = true;
    term;
};

render struct ShoppingCart {
    <div class="shopping-cart">
        <h1>Shopping Cart</h1>
        
        <div class="stats">
            <p>Items: <span b-text="items"></span></p>
            <p>Total: $<span b-text="total"></span></p>
            <p b-show="discount_applied">Discount Applied! (10% off)</p>
        </div>
        
        <div class="actions">
            <button b-trigger:click="add_item(999.99, 1)">
                Add Laptop
            </button>
            <button b-trigger:click="add_item(29.99, 1)">
                Add Mouse
            </button>
        </div>
        
        <div class="controls">
            <button b-trigger:click="remove_item(1)">Remove One</button>
            <button b-trigger:click="clear_cart">Clear Cart</button>
        </div>
    </div>
};
```

## 4. Todo List

```briev
// todo.rbv
struct Todo {
    id: Int;
    text: String;
    completed: Bool;
};

let todos: List<Todo> = [];
let filter: String = "all";

txn add_todo(text: String) [text.^Len > 0][todos.^Len == todos.^Len + 1] {
    let new_todo = Todo {
        id: todos.^Len,
        text: text,
        completed: false
    };
    todos = todos.append(new_todo);
    term;
};

txn toggle_todo(id: Int) [id >= 0 && id < todos.^Len][true] {
    let todo = todos[id];
    todos = todos.set(id, Todo {
        id: todo.id,
        text: todo.text,
        completed: !todo.completed
    });
    term;
};

txn remove_todo(id: Int) [id >= 0 && id < todos.^Len][todos.^Len == todos.^Len - 1] {
    todos = todos.remove(id);
    term;
};

txn clear_completed [true][todos.^Len <= todos.^Len] {
    let filtered: List<Todo> = [];
    let i: Int = 0;
    [i < todos.^Len] {
        [!todos[i].completed] {
            filtered = filtered.append(todos[i]);
        };
        i = i + 1;
    };
    todos = filtered;
    term;
};

render struct TodoList {
    <div class="todo-app">
        <h1>Todo List</h1>
        
        <input type="text" b-bind:value="filter" placeholder="Filter..." />
        <button b-trigger:click="add_todo('New task')">Add</button>
        
        <ul>
            <li b-each:item="todos">
                <input 
                    type="checkbox" 
                    b-trigger:change="toggle_todo(item.id)"
                />
                <span b-text="item.text"></span>
                <button b-trigger:click="remove_todo(item.id)">Delete</button>
            </li>
        </ul>
        
        <button b-trigger:click="clear_completed">Clear Completed</button>
    </div>
};
```

## 5. Traffic Light System

```briev
// traffic_light.bv
enum LightState { Red, Yellow, Green }
let state: LightState = LightState::Red;
let timer: Int = 0;

node change_to_green() 
    [state == LightState::Red && timer >= 60]
    [state == LightState::Green]
{
    state = LightState::Green;
    timer = 0;
    term;
};

node change_to_yellow() 
    [state == LightState::Green && timer >= 30]
    [state == LightState::Yellow]
{
    state = LightState::Yellow;
    timer = 0;
    term;
};

node change_to_red() 
    [state == LightState::Yellow && timer >= 5]
    [state == LightState::Red]
{
    state = LightState::Red;
    timer = 0;
    term;
};

node increment_timer() [true][timer == timer + 1] {
    timer = timer + 1;
    term;
};
```

## 6. Producer-Consumer Pattern

```briev
// producer_consumer.bv
let buffer: List<Int> = [];
let buffer_size: Int = 10;
let produced: Int = 0;
let consumed: Int = 0;

async node produce() 
    [buffer .^Len < buffer_size && produced < 100]
    [produced == produced + 1 && buffer .^Len == buffer .^Len + 1]
{
    produced = produced + 1;
    buffer = buffer.append(produced);
    term;
};

async node consume() 
    [buffer .^Len > 0]
    [consumed == consumed + 1 && buffer .^Len == buffer .^Len - 1]
{
    let item = buffer[0];
    buffer = buffer.drop(1);
    consumed = consumed + 1;
    process(item);
    term;
};

defn process(item: Int) {
    println("Processed: " + String(item));
    term;
};
```

## 7. File Processor with FFI

```briev
// file_processor.bv
import "std/io";
import "std/string";

frgn read_file(path: String) -> Result<String, IOError> from "std/io.bv" fallback "";
frgn write_file(path: String, content: String) -> Result<Void, IOError> from "std/io.bv";

txn process_file(input_path: String, output_path: String) 
    [true]
    [true]
{
    let read_result = read_file(input_path);
    
    [read_result.is_ok()] {
        let content = read_result.value;
        let processed = string.to_upper(content);
        
        let write_result = write_file(output_path, processed);
        [write_result.is_ok()] {
            println("File processed successfully");
        };
        [write_result.is_err()] {
            println("Write error: " + write_result.error.message);
        };
    };
    
    [read_result.is_err()] {
        println("Read error: " + read_result.error.message);
    };
    
    term;
};
```

## 8. State Machine: Vending Machine

```briev
// vending_machine.bv
enum MachineState { Idle, Selection, Payment, Dispensing }
let state: MachineState = MachineState::Idle;
let credit: Int = 0;
let selected_item: String = "";

node insert_coin(amount: Int) 
    [state == MachineState::Idle || state == MachineState::Selection]
    [credit == credit + amount]
{
    credit = credit + amount;
    state = MachineState::Selection;
    term;
};

node select_item(item: String, price: Int) 
    [state == MachineState::Selection && credit >= price]
    [selected_item == item]
{
    selected_item = item;
    state = MachineState::Payment;
    term;
};

node dispense_item() 
    [state == MachineState::Payment]
    [state == MachineState::Idle && credit == credit - price && selected_item == ""]
{
    dispense(selected_item);
    credit = credit - price;
    selected_item = "";
    state = MachineState::Idle;
    term;
};

node refund() 
    [state == MachineState::Selection && credit > 0]
    [state == MachineState::Idle && credit == 0]
{
    refund_money(credit);
    credit = 0;
    state = MachineState::Idle;
    term;
};
```

## 9. Reactive Dashboard

```briev
// dashboard.rbv
let temperature: Float = 20.0;
let humidity: Float = 50.0;
let alerts: List<String> = [];
let last_updated: Int = 0;

node update_sensor_data [true][last_updated == current_time()] {
    temperature = read_temperature();
    humidity = read_humidity();
    last_updated = current_time();
    term;
};

node check_temperature_alert 
    [temperature > 30.0 || temperature < 10.0]
    [alerts.contains("Temperature warning")]
{
    alerts = alerts.append("Temperature warning: " + String(temperature) + "°C");
    term;
};

node check_humidity_alert 
    [humidity > 80.0 || humidity < 20.0]
    [alerts.contains("Humidity warning")]
{
    alerts = alerts.append("Humidity warning: " + String(humidity) + "%");
    term;
};

node clear_old_alerts [alerts.^Len > 10][alerts.^Len <= 10] {
    alerts = alerts.drop(1);
    term;
};

render struct Dashboard {
    <div class="dashboard">
        <h1>Environmental Dashboard</h1>
        
        <div class="sensor">
            <h2>Temperature</h2>
            <p b-text="temperature + '°C'"></p>
        </div>
        
        <div class="sensor">
            <h2>Humidity</h2>
            <p b-text="humidity + '%'"></p>
        </div>
        
        <div class="alerts" b-show="alerts.^Len > 0">
            <h2>Alerts</h2>
            <ul>
                <li b-each:item="alerts" b-text="item"></li>
            </ul>
        </div>
        
        <p class="updated" b-text="'Last updated: ' + last_updated"></p>
    </div>
};
```

## 10. Compile-Time Metaprogramming

Compile-time metaprogramming uses `$(Stage)` blocks with `$let` variables
and `$defn` helper functions:

```briev
// compile_time_meta.bv
$let factor = 1000;

$defn scale(x: Int) -> Int {
    term x * factor;
};

$(Normalized) {
    let val = scale(42);
    EmitInfo$("scaled: " + val);
};
```

`$let` creates mutable compile-time variables; `$const` creates immutable
compile-time constants. `$defn` creates compile-time functions accessible
inside `$(Stage)` blocks. See [16-plugins.md](16-plugins.md) for the full
reference.

---

## Exercises

1. Build a chat application with reactive message updates
2. Create a weather app that fetches data via FFI
3. Implement a reactive stock ticker
4. Build a smart home controller with multiple sensors
5. Create a multiplayer game lobby with real-time updates

---

*Next: [09-patterns.md](09-patterns.md) - Common patterns and best practices*
