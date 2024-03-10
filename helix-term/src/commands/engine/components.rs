use std::{collections::HashMap, sync::Arc};

use helix_core::Position;
use helix_view::{
    graphics::{Color, CursorKind, Rect, UnderlineStyle},
    input::{Event, KeyEvent, MouseEvent},
    keyboard::{KeyCode, KeyModifiers},
    theme::Style,
    Editor,
};
use steel::{
    rvals::{Custom, FromSteelVal, IntoSteelVal, SteelString},
    steel_vm::{builtin::BuiltInModule, engine::Engine, register_fn::RegisterFn},
    SteelVal,
};
use tokio::sync::Mutex;
use tui::{
    buffer::Buffer,
    text::Text,
    widgets::{self, Block, BorderType, Borders, ListItem, Widget},
};

use crate::{
    commands::{
        engine::steel::{BoxDynComponent, ENGINE},
        Context,
    },
    compositor::{self, Component},
    ctrl, key,
    ui::overlay::overlaid,
};

use super::steel::{present_error_inside_engine_context, WrappedDynComponent};

#[derive(Clone)]
struct AsyncReader {
    // Take that, and write it back to a terminal session that is
    // getting rendered.
    channel: Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<String>>>,
}

impl AsyncReader {
    // TODO: Add &mut references to these async functions
    // to avoid the cloning, and to ditch the arc and mutex
    async fn read_line(self) -> Option<String> {
        let mut buf = String::new();

        let mut guard = self.channel.lock().await;

        while let Ok(v) = guard.try_recv() {
            buf.push_str(&v);
        }

        let fut = guard.recv();

        match tokio::time::timeout(std::time::Duration::from_millis(2), fut).await {
            Ok(Some(v)) => {
                buf.push_str(&v);
                Some(buf)
            }
            Ok(None) => {
                if buf.is_empty() {
                    None
                } else {
                    Some(buf)
                }
            }
            Err(_) => Some(buf),
        }
    }
}

impl Custom for AsyncReader {}

struct AsyncWriter {
    channel: tokio::sync::mpsc::UnboundedSender<String>,
}

impl std::io::Write for AsyncWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Err(_) = self.channel.send(String::from_utf8_lossy(buf).to_string()) {
            Ok(0)
        } else {
            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// TODO: Move the main configuration function to use this instead
pub fn helix_component_module() -> BuiltInModule {
    let mut module = BuiltInModule::new("helix/components");

    module
        .register_fn("async-read-line", AsyncReader::read_line)
        // TODO:
        .register_fn("make-async-reader-writer", || {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

            let writer = AsyncWriter { channel: sender };
            let reader = AsyncReader {
                channel: Arc::new(Mutex::new(receiver)),
            };

            vec![
                SteelVal::new_dyn_writer_port(writer),
                reader.into_steelval().unwrap(),
            ]
        })
        // Attempt to pop off a specific component
        .register_fn(
            "pop-dynamic-component-by-name",
            |ctx: &mut Context, name: SteelString| {
                // Removing a component by name here will be important!
                todo!()
            },
        )
        .register_fn("buffer-area", |buffer: &mut Buffer| buffer.area)
        .register_fn("frame-set-string!", buffer_set_string)
        .register_fn("new-component!", SteelDynamicComponent::new_dyn)
        .register_fn("position", Position::new)
        .register_fn("position-row", |position: &Position| position.row)
        .register_fn("position-col", |position: &Position| position.col)
        .register_fn(
            "set-position-row!",
            |position: &mut Position, row: usize| {
                position.row = row;
            },
        )
        .register_fn(
            "set-position-col!",
            |position: &mut Position, col: usize| {
                position.col = col;
            },
        )
        .register_fn("area", helix_view::graphics::Rect::new)
        .register_fn("area-x", |area: &helix_view::graphics::Rect| area.x)
        .register_fn("area-y", |area: &helix_view::graphics::Rect| area.y)
        .register_fn("area-width", |area: &helix_view::graphics::Rect| area.width)
        .register_fn("area-height", |area: &helix_view::graphics::Rect| {
            area.height
        })
        .register_fn("overlaid", |component: &mut WrappedDynComponent| {
            let inner: Option<Box<dyn Component>> = component
                .inner
                .take()
                .map(|x| Box::new(overlaid(BoxDynComponent::new(x))) as Box<dyn Component>);

            component.inner = inner;
        })
        .register_fn("widget/list", |items: Vec<String>| {
            widgets::List::new(
                items
                    .into_iter()
                    .map(|x| ListItem::new(Text::from(x)))
                    .collect::<Vec<_>>(),
            )
        })
        // Pass references in as well?
        .register_fn(
            "widget/list/render",
            |buf: &mut Buffer, area: Rect, list: widgets::List| list.render(area, buf),
        )
        .register_fn("block", || {
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::White))
                .border_type(BorderType::Rounded)
                .style(Style::default().bg(Color::Black))
        })
        .register_fn(
            "block/render",
            |buf: &mut Buffer, area: Rect, block: Block| block.render(area, buf),
        )
        .register_fn("buffer/clear", Buffer::clear)
        .register_fn("buffer/clear-with", Buffer::clear_with)
        // Mutate a color in place, to save some headache.
        .register_fn(
            "set-color-rgb!",
            |color: &mut Color, r: u8, g: u8, b: u8| {
                *color = Color::Rgb(r, g, b);
            },
        )
        .register_fn("set-color-indexed!", |color: &mut Color, index: u8| {
            *color = Color::Indexed(index);
        })
        .register_value("Color/Reset", Color::Reset.into_steelval().unwrap())
        .register_value("Color/Black", Color::Black.into_steelval().unwrap())
        .register_value("Color/Red", Color::Red.into_steelval().unwrap())
        .register_value("Color/White", Color::White.into_steelval().unwrap())
        .register_value("Color/Green", Color::Green.into_steelval().unwrap())
        .register_value("Color/Yellow", Color::Yellow.into_steelval().unwrap())
        .register_value("Color/Blue", Color::Blue.into_steelval().unwrap())
        .register_value("Color/Magenta", Color::Magenta.into_steelval().unwrap())
        .register_value("Color/Cyan", Color::Cyan.into_steelval().unwrap())
        .register_value("Color/Gray", Color::Gray.into_steelval().unwrap())
        .register_value("Color/LightRed", Color::LightRed.into_steelval().unwrap())
        .register_value(
            "Color/LightGreen",
            Color::LightGreen.into_steelval().unwrap(),
        )
        .register_value(
            "Color/LightYellow",
            Color::LightYellow.into_steelval().unwrap(),
        )
        .register_value("Color/LightBlue", Color::LightBlue.into_steelval().unwrap())
        .register_value(
            "Color/LightMagenta",
            Color::LightMagenta.into_steelval().unwrap(),
        )
        .register_value("Color/LightCyan", Color::LightCyan.into_steelval().unwrap())
        .register_value("Color/LightGray", Color::LightGray.into_steelval().unwrap())
        .register_fn("Color/rgb", Color::Rgb)
        .register_fn("Color/Indexed", Color::Indexed)
        .register_fn("set-style-fg!", |style: &mut Style, color: Color| {
            style.fg = Some(color);
        })
        .register_fn("style-fg", Style::fg)
        .register_fn("style-bg", Style::bg)
        .register_fn("set-style-bg!", |style: &mut Style, color: Color| {
            style.bg = Some(color);
        })
        .register_fn("style-underline-color", Style::underline_color)
        .register_fn("style-underline-style", Style::underline_style)
        .register_value(
            "Underline/Reset",
            UnderlineStyle::Reset.into_steelval().unwrap(),
        )
        .register_value(
            "Underline/Line",
            UnderlineStyle::Line.into_steelval().unwrap(),
        )
        .register_value(
            "Underline/Curl",
            UnderlineStyle::Curl.into_steelval().unwrap(),
        )
        .register_value(
            "Underline/Dotted",
            UnderlineStyle::Dotted.into_steelval().unwrap(),
        )
        .register_value(
            "Underline/Dashed",
            UnderlineStyle::Dashed.into_steelval().unwrap(),
        )
        .register_value(
            "Underline/DoubleLine",
            UnderlineStyle::DoubleLine.into_steelval().unwrap(),
        )
        .register_fn("style", || Style::default())
        .register_value(
            "event-result/consume",
            SteelEventResult::Consumed.into_steelval().unwrap(),
        )
        .register_value(
            "event-result/ignore",
            SteelEventResult::Ignored.into_steelval().unwrap(),
        )
        .register_value(
            "event-result/close",
            SteelEventResult::Close.into_steelval().unwrap(),
        )
        // TODO: Use a reference here instead of passing by value.
        .register_fn("key-event-char", |event: Event| {
            if let Event::Key(event) = event {
                event.char()
            } else {
                None
            }
        })
        .register_fn("key-event-modifier", |event: Event| {
            if let Event::Key(KeyEvent { modifiers, .. }) = event {
                Some(modifiers.bits())
            } else {
                None
            }
        })
        .register_value(
            "key-modifier-ctrl",
            SteelVal::IntV(KeyModifiers::CONTROL.bits() as isize),
        )
        .register_value(
            "key-modifier-shift",
            SteelVal::IntV(KeyModifiers::SHIFT.bits() as isize),
        )
        .register_value(
            "key-modifier-alt",
            SteelVal::IntV(KeyModifiers::ALT.bits() as isize),
        )
        .register_fn("key-event-F?", |event: Event, number: u8| match event {
            Event::Key(KeyEvent {
                code: KeyCode::F(x),
                ..
            }) if number == x => true,
            _ => false,
        })
        .register_fn("mouse-event?", |event: Event| {
            matches!(event, Event::Mouse(_))
        })
        .register_fn("event-mouse-kind", |event: Event| {
            if let Event::Mouse(MouseEvent { .. }) = event {
                todo!()
            } else {
                todo!()
            }
        })
        .register_fn("event-mouse-row", |event: Event| {
            if let Event::Mouse(MouseEvent { .. }) = event {
                todo!()
            } else {
                todo!()
            }
        })
        .register_fn("event-mouse-col", |event: Event| {
            if let Event::Mouse(MouseEvent { .. }) = event {
                todo!()
            } else {
                todo!()
            }
        })
        // Is this mouse event within the area provided
        .register_fn("mouse-event-within-area?", |event: Event, area: Rect| {
            if let Event::Mouse(MouseEvent { row, column, .. }) = event {
                column > area.x
                    && column < area.x + area.width
                    && row > area.y
                    && row < area.y + area.height
            } else {
                false
            }
        });

    macro_rules! register_key_events {
        ($ ( $name:expr => $key:tt ) , *, ) => {
            $(
              module.register_fn(concat!("key-event-", $name, "?"), |event: Event| {
                  matches!(
                      event,
                      Event::Key(
                          KeyEvent {
                              code: KeyCode::$key,
                              ..
                          }
                      ))
                  });
            )*
        };
    }

    // Key events for individual key codes
    register_key_events!(
        "escape" => Esc,
        "backspace" => Backspace,
        "enter" => Enter,
        "left" => Left,
        "right" => Right,
        "up" => Up,
        "down" => Down,
        "home" => Home,
        "page-up" => PageUp,
        "page-down" => PageDown,
        "tab" => Tab,
        "delete" => Delete,
        "insert" => Insert,
        "null" => Null,
        "caps-lock" => CapsLock,
        "scroll-lock" => ScrollLock,
        "num-lock" => NumLock,
        "print-screen" => PrintScreen,
        "pause" => Pause,
        "menu" => Menu,
        "keypad-begin" => KeypadBegin,
    );

    module
}

fn buffer_set_string(
    buffer: &mut tui::buffer::Buffer,
    x: u16,
    y: u16,
    string: steel::rvals::SteelString,
    style: Style,
) {
    buffer.set_string(x, y, string.as_str(), style)
}

/// A dynamic component, used for rendering
#[derive(Clone)]
pub struct SteelDynamicComponent {
    // TODO: currently the component id requires using a &'static str,
    // however in a world with dynamic components that might not be
    // the case anymore
    _name: String,
    // This _should_ be a struct, but in theory can be whatever you want. It will be the first argument
    // passed to the functions in the remainder of the struct.
    state: SteelVal,
    handle_event: Option<SteelVal>,
    should_update: Option<SteelVal>,
    render: SteelVal,
    cursor: Option<SteelVal>,
    required_size: Option<SteelVal>,

    // Cached key event; we keep this around so that when sending
    // events to the event handler, we can reuse the heap allocation
    // instead of re-allocating for every event (which might be a lot)
    key_event: Option<SteelVal>,
}

impl SteelDynamicComponent {
    pub fn new(
        name: String,
        state: SteelVal,
        render: SteelVal,
        h: HashMap<String, SteelVal>,
    ) -> Self {
        Self {
            _name: name,
            state,
            render,
            handle_event: h.get("handle_event").cloned(),
            should_update: h.get("should_update").cloned(),
            cursor: h.get("cursor").cloned(),
            required_size: h.get("required_size").cloned(),
            key_event: None,
        }
    }

    pub fn new_dyn(
        name: String,
        state: SteelVal,
        render: SteelVal,
        h: HashMap<String, SteelVal>,
    ) -> WrappedDynComponent {
        let s = Self::new(name, state, render, h);

        // TODO: Add guards here for the
        WrappedDynComponent {
            inner: Some(Box::new(s)),
        }
    }

    pub fn get_state(&self) -> SteelVal {
        self.state.clone()
    }

    pub fn get_render(&self) -> SteelVal {
        self.render.clone()
    }

    pub fn get_handle_event(&self) -> Option<SteelVal> {
        self.handle_event.clone()
    }

    pub fn get_should_update(&self) -> Option<SteelVal> {
        self.should_update.clone()
    }

    pub fn get_cursor(&self) -> Option<SteelVal> {
        self.cursor.clone()
    }

    pub fn get_required_size(&self) -> Option<SteelVal> {
        self.required_size.clone()
    }
}

impl Custom for SteelDynamicComponent {}

impl Custom for Box<dyn Component> {}

#[derive(Clone)]
enum SteelEventResult {
    Consumed,
    Ignored,
    Close,
}

impl Custom for SteelEventResult {}

impl Component for SteelDynamicComponent {
    fn render(
        &mut self,
        area: helix_view::graphics::Rect,
        frame: &mut tui::buffer::Buffer,
        ctx: &mut compositor::Context,
    ) {
        let mut ctx = Context {
            register: None,
            count: None,
            editor: ctx.editor,
            callback: Vec::new(),
            on_next_key_callback: None,
            jobs: ctx.jobs,
        };

        // Pass the `state` object through - this can be used for storing the state of whatever plugin thing we're
        // attempting to render
        let thunk = |engine: &mut Engine, f| {
            engine.call_function_with_args_from_mut_slice(
                self.render.clone(),
                &mut [self.state.clone(), area.into_steelval().unwrap(), f],
            )
        };

        ENGINE.with(|x| {
            let mut guard = x.borrow_mut();

            if let Err(e) = guard
                .with_mut_reference::<tui::buffer::Buffer, tui::buffer::Buffer>(frame)
                .with_mut_reference::<Context, Context>(&mut ctx)
                .consume(|engine, args| {
                    let mut arg_iter = args.into_iter();

                    let buffer = arg_iter.next().unwrap();
                    let context = arg_iter.next().unwrap();

                    engine.update_value("*helix.cx*", context);

                    (thunk)(engine, buffer)
                })
            {
                present_error_inside_engine_context(&mut ctx, &mut guard, e)
            }
        })
    }

    // TODO: Pass in event as well? Need to have immutable reference type
    // Otherwise, we're gonna be in a bad spot. For now - just clone the object and pass it through.
    // Clong is _not_ ideal, but it might be all we can do for now.
    fn handle_event(
        &mut self,
        event: &Event,
        ctx: &mut compositor::Context,
    ) -> compositor::EventResult {
        if let Some(handle_event) = &mut self.handle_event {
            let mut ctx = Context {
                register: None,
                count: None,
                editor: ctx.editor,
                callback: Vec::new(),
                on_next_key_callback: None,
                jobs: ctx.jobs,
            };

            match self.key_event.as_mut() {
                Some(SteelVal::Custom(key_event)) => {
                    // Save the headache, reuse the allocation
                    if let Some(inner) = steel::rvals::as_underlying_type_mut::<Event>(
                        key_event.borrow_mut().as_mut(),
                    ) {
                        *inner = event.clone();
                    }
                }

                None => {
                    self.key_event = Some(event.clone().into_steelval().unwrap());
                }
                _ => {
                    panic!("This event needs to stay as a steelval");
                }
            }

            // Pass the `state` object through - this can be used for storing the state of whatever plugin thing we're
            // attempting to render
            let thunk = |engine: &mut Engine| {
                engine.call_function_with_args_from_mut_slice(
                    handle_event.clone(),
                    &mut [self.state.clone(), self.key_event.clone().unwrap()],
                )
            };

            let close_fn = compositor::EventResult::Consumed(Some(Box::new(
                |compositor: &mut compositor::Compositor, _| {
                    // remove the layer
                    compositor.pop();
                },
            )));

            let event = match event {
                Event::Key(event) => *event,
                _ => return compositor::EventResult::Ignored(None),
            };

            match ENGINE.with(|x| {
                x.borrow_mut()
                    .with_mut_reference::<Context, Context>(&mut ctx)
                    .consume(move |engine, arguments| {
                        let context = arguments[0].clone();
                        engine.update_value("*helix.cx*", context);

                        thunk(engine)
                    })
            }) {
                Ok(v) => {
                    let value = SteelEventResult::from_steelval(&v);

                    match value {
                        Ok(SteelEventResult::Close) => close_fn,
                        Ok(SteelEventResult::Consumed) => compositor::EventResult::Consumed(None),
                        Ok(SteelEventResult::Ignored) => compositor::EventResult::Ignored(None),
                        _ => match event {
                            ctrl!('c') | key!(Esc) => close_fn,
                            _ => compositor::EventResult::Ignored(None),
                        },
                    }
                }
                Err(e) => {
                    // Present the error
                    ENGINE.with(|x| {
                        present_error_inside_engine_context(&mut ctx, &mut x.borrow_mut(), e)
                    });

                    compositor::EventResult::Ignored(None)
                }
            }
        } else {
            compositor::EventResult::Ignored(None)
        }
    }

    fn should_update(&self) -> bool {
        true

        // if let Some(should_update) = &self.should_update {
        //     match ENGINE.with(|x| {
        //         let res = x
        //             .borrow_mut()
        //             .call_function_with_args(should_update.clone(), vec![self.state.clone()]);

        //         res
        //     }) {
        //         Ok(v) => bool::from_steelval(&v).unwrap_or(true),
        //         Err(_) => true,
        //     }
        // } else {
        //     true
        // }
    }

    // TODO: Implement immutable references. Right now I'm only supporting mutable references.
    fn cursor(
        &self,
        area: helix_view::graphics::Rect,
        _ctx: &Editor,
    ) -> (
        Option<helix_core::Position>,
        helix_view::graphics::CursorKind,
    ) {
        if let Some(cursor) = &self.cursor {
            // Pass the `state` object through - this can be used for storing the state of whatever plugin thing we're
            // attempting to render
            let thunk = |engine: &mut Engine| {
                engine.call_function_with_args(
                    cursor.clone(),
                    vec![self.state.clone(), area.into_steelval().unwrap()],
                )
            };

            let result = Option::<helix_core::Position>::from_steelval(
                &ENGINE.with(|x| thunk(&mut (x.borrow_mut())).unwrap()),
            );

            match result {
                Ok(v) => (v, CursorKind::Block),
                // TODO: Figure out how to pop up an error message
                Err(_e) => (None, CursorKind::Block),
            }
        } else {
            (None, helix_view::graphics::CursorKind::Hidden)
        }
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        let name = self.type_name();

        if let Some(required_size) = &mut self.required_size {
            log::info!("Calling required-size inside: {}", name);

            // TODO: Create some token that we can grab to enqueue function calls internally. Referencing
            // the external API would cause problems - we just need to include a handle to the interpreter
            // instance. Something like:
            // ENGINE.call_function_or_enqueue? OR - this is the externally facing render function. Internal
            // render calls do _not_ go through this interface. Instead, they are just called directly.
            //
            // If we go through this interface, we're going to get an already borrowed mut error, since it is
            // re-entrant attempting to grab the ENGINE instead mutably, since we have to break the recursion
            // somehow. By putting it at the edge, we then say - hey for these functions on this interface,
            // call the engine instance. Otherwise, all computation happens inside the engine.
            match ENGINE
                .with(|x| {
                    x.borrow_mut().call_function_with_args(
                        required_size.clone(),
                        vec![self.state.clone(), viewport.into_steelval().unwrap()],
                    )
                })
                .and_then(|x| Option::<(u16, u16)>::from_steelval(&x))
            {
                Ok(v) => v,
                // TODO: Figure out how to present an error
                Err(_e) => None,
            }
        } else {
            None
        }
    }

    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn id(&self) -> Option<&'static str> {
        None
    }
}
