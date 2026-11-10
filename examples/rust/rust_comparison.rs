// Тот же код, что examples/tour.keel, но на Rust.
// Цель файла — показать, что он НЕ компилируется: ровно те места,
// которые в Keel обычны, в Rust запрещены.

struct Entity {
    hp: i32,
    name: String,
}

struct World {
    entities: Vec<Entity>,
    alive: i32,
}

// Rust: не компилируется.
// cannot borrow `world` as immutable because it is also borrowed as mutable
fn step(world: &World, entity: &mut Entity, amount: i32) {
    entity.hp -= amount;
}

// Rust: error[E0499] cannot borrow as mutable more than once
fn heal_first_and_second(world: &mut World) {
    let a = &mut world.entities[0];
    let b = &mut world.entities[1];
    a.hp += 10;
    b.hp += 10;
}

fn main() {
    let mut world = World {
        entities: vec![
            Entity { hp: 100, name: "orc".into() },
            Entity { hp: 80, name: "goblin".into() },
            Entity { hp: 50, name: "rat".into() },
        ],
        alive: 3,
    };
    step(&world, &mut world.entities[0], 30);
    heal_first_and_second(&mut world);
}
