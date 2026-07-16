# Why the login dialog closed on Enter

## Symptom

Pressing Enter in the auth dialog closed the dialog immediately instead of submitting the form and waiting for the login result.

## Root cause

gpui-component's `Dialog` registers a default key binding for Enter:

```rust
KeyBinding::new("enter", ConfirmDialog, Some(CONTEXT))
```

When Enter is pressed inside a dialog, the `ConfirmDialog` action fires and runs the dialog's `on_ok` callback. The default `on_ok` returns `true`, which makes the dialog call `defer_close_dialog()` and close itself.

This happens **regardless of whether the focus is in an input, a button, or the dialog content**. So even though we added submit logic on Enter, the dialog's built-in action always won and closed the modal.

## Why earlier attempts did not work

### Attempt 1: `on_key_down` on the dialog content

Adding `.on_key_down(...)` on the content container fired our submit handler, but the raw key event still propagated up to the dialog, triggering `ConfirmDialog` and closing the modal.

Calling `cx.stop_propagation()` inside the handler did not reliably prevent the dialog-level action from firing because actions are dispatched through GPUI's key-binding system, not only through DOM-style event bubbling.

### Attempt 2: `InputEvent::PressEnter`

`gpui_component::input::InputState` emits `InputEvent::PressEnter` when Enter is pressed. Subscribing to it did call our submit logic, but the input also calls `cx.propagate()` for single-line inputs, so the raw Enter event still reached the dialog and triggered `ConfirmDialog`.

## Fix

Use the dialog's own `.on_ok()` API to intercept the `ConfirmDialog` action:

```rust
window.open_dialog(cx, move |dialog, _, _| {
    let view_for_ok = view.clone();
    dialog
        .on_ok(move |_, window, cx| {
            view_for_ok.update(cx, |view, _cx| {
                view.submit(window, _cx);
            });
            false // do NOT let the dialog close automatically
        })
        ...
})
```

Returning `false` from `on_ok` prevents `defer_close_dialog()` from running. The dialog now stays open until the async login/signup task succeeds and explicitly calls `window.close_dialog(cx)`.

## Reference

Dialog source (git rev `7315e07`):

```rust
// crates/ui/src/dialog/dialog.rs
KeyBinding::new("enter", ConfirmDialog, Some(CONTEXT))

...

.on_action({
    let on_ok = on_ok.clone();
    let on_close = on_close.clone();
    move |_: &ConfirmDialog, window, cx| {
        if on_ok(&ClickEvent::default(), window, cx) {
            Self::defer_close_dialog(window, cx);
            on_close(&ClickEvent::default(), window, cx);
        }
    }
})
```
