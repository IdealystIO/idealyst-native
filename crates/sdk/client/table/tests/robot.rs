//! The web `<table>` lowering through the robot registry.
//!
//! Regression: an idea-ui Table's clickable rows could not be pressed by the
//! robot. On web the rows lower to `<tr>`/`<td>` scene-Registry items whose
//! handlers never registered with the robot, so a cell's text parented
//! straight to whatever sat above the table and the cell's tap recognizer
//! (`set_cell_interaction`) was unreachable — `click` reported "not
//! available" on everything in the row.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use backend_ssr::SsrBackend;
use runtime_scene::{realize, Registry};
use runtime_vocabulary::glue::{self, IntoElement};
use runtime_vocabulary::robot::{Query, Robot};
use runtime_world::World;
use table::{item_lowering, set_cell_interaction, ColSpan};

#[test]
fn regression_robot_click_presses_a_web_table_row() {
    let robot = Robot::new();
    robot.reset();
    let backend = Rc::new(RefCell::new(SsrBackend::new()));
    let mut registry: Registry<SsrBackend> = Registry::new();
    runtime_vocabulary::register_builtins_with::<_, runtime_vocabulary::AllBuiltins>(&mut registry);
    table::register(&mut registry);
    let registry = Rc::new(registry);
    let world = World::new();

    let pressed = Rc::new(Cell::new(0));
    let _realized = world.enter(|| {
        let cell = item_lowering::cell_item(
            false,
            ColSpan::default(),
            vec![glue::text("New Hire Intake").into_element()],
            None,
        );
        let sink = pressed.clone();
        set_cell_interaction(
            &cell,
            runtime_shared::tap(runtime_shared::TapRecognizer::new(), move || {
                sink.set(sink.get() + 1)
            }),
            Rc::new(|_| {}),
        );
        let element =
            item_lowering::table_item(vec![item_lowering::row_item(vec![cell], None)], None, false);
        realize(&backend, &registry, element)
    });

    let label = robot.find(Query::label("New Hire Intake")).expect("cell text registered");
    // The text now sits inside the cell → row → table chain.
    let td = robot.parent_of(&label).expect("text parents to its <td>");
    let tr = robot.parent_of(&td).expect("<td> parents to its <tr>");
    let tbl = robot.parent_of(&tr).expect("<tr> parents to the <table>");
    assert!(robot.parent_of(&tbl).is_none(), "the <table> is the root here");

    robot.click(&label).expect("click on the row's text reaches the row handler");
    assert_eq!(pressed.get(), 1);
    robot.reset();
}
