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
cargo run -p ratatui-hypertile-extras --example basic --release

# core only, no extras dependency
cargo run --example core_only
```

## Keys

### General

| Key            | Operation |
| -------------- | --------- |
| Ctrl + Alt + c | quit      |

### Block

| Key                   | Operation                                                             |
| --------------------- | --------------------------------------------------------------------- |
| Alt + p               | open palette                                                          |
| Alt + q               | close focused block                                                   |
| Alt + d               | toggle maximize                                                       |
| Alt + e               | open fish                                                             |
| Alt + t               | split focused block automatically                                     |
| Alt + s/v             | split focused block horizontally/vertically                           |
| Alt + -/=             | resize focused block                                                  |
| Alt + h/j/k/l         | focus                                                                 |
| Alt + Shift + h/j/k/l | move block                                                            |
| Ctrl + g              | toggle transparent input (means every key will be sent to this block) |

### Workspace

| Key            | Operation                    |
| -------------- | ---------------------------- |
| Ctrl + Alt + t | create new workspace         |
| Ctrl + Alt + w | close current workspace      |
| Alt + 0-9      | switch to specific workspace |
| Alt + ←/→      | switch workspace             |

### Unimplemented (will support soon)

| Key               | Operation                    |
| ----------------- | ---------------------------- |
| Alt + Shift + 0-9 | send to specific workspace   |
| Alt + Shift + ←/→ | send to left/right workspace |
| Alt + f           | toggle fullscreen            |
| Alt + /           | toggle cheatsheet            |
| Alt + j           | toggle top bar               |

## License

This project is licensed under the [MIT License](LICENSE).
