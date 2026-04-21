![demo](assets/demo.gif)

Cook up delicious terminal interfaces with Hyprland-style tiling for [Ratatui](https://github.com/ratatui/ratatui). Splits, tabs, animations, persistence.

Originated from this [repo](https://github.com/nikolic-milos/ratatui-hypertile), but I add more animation and want to make it something like `tmux`

## What's in the box

[`ratatui-hypertile`](https://crates.io/crates/ratatui-hypertile) is the core engine. You give it an area, it gives you rectangles. Handles the tree, focus, movement. Use this when you want full control.

[`ratatui-hypertile-extras`](https://crates.io/crates/ratatui-hypertile-extras) wraps the core into a runtime with plugins, vim keymaps, a command palette, workspace tabs and pane-move animations. Implement `HypertilePlugin` and you're set.

## Try it out

From the repo root:

```sh
# full runtime: plugins, tabs, palette, animations
cargo run -p ratatui-hypertile-extras --example basic

# core only, no extras dependency
cargo run --example core_only
```

**Keys:**

### General

| Key      | Operation |
| -------- | --------- |
| Ctrl + c | quit      |

### Block

| Key             | Operation                                   |
| --------------- | ------------------------------------------- |
| p               | open new block                              |
| d               | close focused block                         |
| f               | toggle fullscreen                           |
| t               | split focused block automatically           |
| s/v             | split focused block horizontally/vertically |
| [ / ]           | resize focused block                        |
| h/j/k/l         | focus                                       |
| Shift + h/j/k/l | move block                                  |

### Workspace

| Key       | Operation                    |
| --------- | ---------------------------- |
| Ctrl + t  | create new workspace         |
| Ctrl + w  | close current workspace      |
| 0-9       | switch to specific workspace |
| Alt + ←/→ | switch workspace             |

## Quickstart

```toml
# just the layout engine
ratatui-hypertile = "0.3"

# or the full runtime with plugins
ratatui-hypertile-extras = "0.3"
```

```rust
use ratatui::layout::Direction;
use ratatui_hypertile::Hypertile;

let mut layout = Hypertile::new();
let pane = layout.split_focused(Direction::Horizontal).unwrap();

layout.compute_layout(area);
for pane in layout.panes_iter() {
    // pane.id, pane.rect, pane.is_focused
}
```

## License

This project is licensed under the [MIT License](LICENSE).
