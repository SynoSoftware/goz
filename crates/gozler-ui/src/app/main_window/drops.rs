//! Drag feedback may be replaced; accepted source intake keeps its operation id.

use super::*;
use crate::app::work::drops::{Answer, Evaluated, Evaluation, Expired};

pub(super) struct Dropping {
    destination: PathBuf,
    moves: bool,
    source: DropSource,
}

pub(super) struct DragIn {
    landed: bool,
    preview: Option<Id>,
    dragged: Option<Arc<Dragged>>,
    evaluation: Option<(Id, EffectKey)>,
    evaluated: Option<(EffectKey, Evaluated)>,
    pub(super) target: Option<DropFolder>,
    pub(super) over: Option<((i32, i32), DropKeys, SourceEffects)>,
    pub(super) scrolling: Option<ScrollEdge>,
}

#[derive(PartialEq)]
struct EffectKey {
    row: u64,
    folder: ResultItem,
    keys: DropKeys,
    allowed: SourceEffects,
}

impl DragIn {
    fn expire(&mut self, expired: Expired) -> bool {
        if self.landed {
            return false;
        }
        match expired {
            Expired::Preview(id) if self.preview == Some(id) => self.preview = None,
            Expired::Evaluation(id)
                if self
                    .evaluation
                    .as_ref()
                    .is_some_and(|(current, _)| *current == id) =>
            {
                self.evaluation = None
            }
            _ => return false,
        }
        self.over = None;
        self.evaluated = None;
        true
    }
}

impl MainWindow {
    pub(super) fn expire_hover(&self, expired: &[Expired]) {
        let changed = {
            let mut drag_in = self.drag_in.borrow_mut();
            drag_in.as_mut().and_then(|drag| {
                let mut changed = false;
                for &expired in expired {
                    changed |= drag.expire(expired);
                }
                changed.then(|| {
                    (
                        drag.target.take().map(|target| target.row),
                        drag.scrolling.take().is_some(),
                    )
                })
            })
        };
        let Some((row, scrolled)) = changed else {
            return;
        };
        if scrolled {
            self.created()
                .controls
                .list
                .hwnd()
                .parent()
                .kill_timer(timer::DROP_SCROLL);
        }
        self.redraw_drop_rows(row);
    }

    pub(super) fn abandon_drop(&self, id: OperationId) {
        let source = self.dropping.borrow_mut().remove(&id);
        drop(source);
    }

    pub(super) fn finish_drops(&self) {
        let (preview, effect, answer, retired) = {
            let mut work = self.created().work.borrow_mut();
            (
                work.drops.take_preview(),
                work.drops.take_effect(),
                work.drops.take(),
                work.drops.take_retired(),
            )
        };
        self.created().work.borrow_mut().retire(retired);
        let over = {
            let mut drag_in = self.drag_in.borrow_mut();
            drag_in.as_mut().and_then(|drag| {
                if drag.landed {
                    return None;
                }
                if let Some((id, paths)) = preview
                    && drag.preview == Some(id)
                {
                    drag.dragged = Some(paths);
                    drag.preview = None;
                }
                if let Some((id, answer)) = effect
                    && drag
                        .evaluation
                        .as_ref()
                        .is_some_and(|(current, _)| *current == id)
                {
                    let (_, key) = drag.evaluation.take().expect("the evaluation matched");
                    drag.evaluated = Some((key, answer));
                }
                drag.over
            })
        };
        if let Some((point, keys, allowed)) = over {
            self.drag_over(point, keys, allowed);
        }
        match answer {
            Some(Answer::Setup(id, prepared)) => {
                let mut prepared = prepared;
                let exists = {
                    let mut dropping = self.dropping.borrow_mut();
                    if let Some(drop) = dropping.get_mut(&id) {
                        if let Some(prepared) = &mut prepared {
                            drop.source.prepared(prepared);
                        }
                        true
                    } else {
                        false
                    }
                };
                if exists {
                    if let Some(prepared) = prepared {
                        self.created()
                            .work
                            .borrow_mut()
                            .drops
                            .read(id, prepared.data);
                    } else {
                        self.finish_drop(id, Vec::new());
                    }
                }
            }
            Some(Answer::Read(id, paths)) => self.finish_drop(id, paths),
            None => {}
        }
    }

    fn finish_drop(&self, id: OperationId, paths: Vec<PathBuf>) {
        let dropped = {
            let mut dropping = self.dropping.borrow_mut();
            dropping.remove(&id)
        };
        if let Some(dropped) = dropped {
            self.resolve_transfer(
                id,
                paths,
                dropped.destination,
                dropped.moves,
                Some(FilesSource::Drop(dropped.source)),
            );
        } else {
            self.retire_paths(paths);
        }
    }
}

impl DropHandler for MainWindow {
    fn drag_enter(&self, data: Option<DropData>) {
        self.drag_leave();
        let preview = data.map(|data| self.created().work.borrow_mut().drops.preview(data));
        self.drag_in.replace(Some(DragIn {
            landed: false,
            preview,
            dragged: None,
            evaluation: None,
            evaluated: None,
            target: None,
            over: None,
            scrolling: None,
        }));
    }

    fn drag_over(
        &self,
        point: (i32, i32),
        keys: DropKeys,
        allowed: SourceEffects,
    ) -> Option<DropEffect> {
        if let Some(drag) = self.drag_in.borrow().as_ref()
            && drag.landed
        {
            return drag
                .evaluated
                .as_ref()
                .and_then(|(_, answer)| answer.effect);
        }
        let list = self.created().controls.list;
        let accepts = self.model.borrow().mode() == Mode::Search
            && !self.modal_running.get()
            && self.file_operations.get().is_some();
        let row = (accepts && list.hwnd().screen_rect().contains(point) && !self.on_header(point))
            .then(|| list.row_at(point))
            .flatten();
        let shown = row.and_then(|row| {
            self.model
                .borrow()
                .shown_row(List::Search, row)
                .cloned()
                .map(|folder| (row, folder))
        });
        let edge = list.drag_scroll_edge(point).filter(|_| accepts);
        let (effect, changed, scrolling) = {
            let mut drag_in = self.drag_in.borrow_mut();
            let drag = drag_in.as_mut()?;
            if drag.landed {
                return drag
                    .evaluated
                    .as_ref()
                    .and_then(|(_, answer)| answer.effect);
            }
            let effect = match (shown.as_ref(), drag.dragged.as_ref()) {
                (Some((row, folder)), Some(dragged)) => {
                    let key = EffectKey {
                        row: *row,
                        folder: folder.clone(),
                        keys,
                        allowed,
                    };
                    if drag
                        .evaluated
                        .as_ref()
                        .is_some_and(|(prior, _)| *prior == key)
                    {
                        drag.evaluated
                            .as_ref()
                            .and_then(|(_, answer)| answer.effect)
                    } else {
                        if !drag
                            .evaluation
                            .as_ref()
                            .is_some_and(|(_, prior)| *prior == key)
                        {
                            let id = self.created().work.borrow_mut().drops.evaluate(Evaluation {
                                dragged: Arc::clone(dragged),
                                folder: folder.clone(),
                                keys,
                                allowed,
                            });
                            drag.evaluation = Some((id, key));
                        }
                        None
                    }
                }
                _ => None,
            };
            let target = shown
                .filter(|_| effect.is_some())
                .map(|(row, folder)| DropFolder { row, folder });
            let before = drag.target.as_ref().map(|target| target.row);
            let after = target.as_ref().map(|target| target.row);
            drag.target = target;
            drag.over = Some((point, keys, allowed));
            let scrolling = std::mem::replace(&mut drag.scrolling, edge);
            (
                effect,
                (before != after).then_some([before, after]),
                scrolling,
            )
        };
        if let Some(rows) = changed {
            self.redraw_drop_rows(rows.into_iter().flatten());
        }
        if self
            .drag_in
            .borrow()
            .as_ref()
            .is_some_and(|drag| drag.landed)
        {
            return effect;
        }
        let window = list.hwnd().parent();
        match (scrolling, edge) {
            (None, Some(_)) => window.set_timer(timer::DROP_SCROLL, DROP_SCROLL_MS),
            (Some(_), None) => window.kill_timer(timer::DROP_SCROLL),
            _ => {}
        }
        effect
    }

    fn hold_still(&self) {
        let scrolled = self.drag_in.borrow_mut().as_mut().is_some_and(|drag| {
            drag.landed = true;
            drag.over = None;
            drag.preview = None;
            drag.evaluation = None;
            drag.scrolling.take().is_some()
        });
        self.created().work.borrow_mut().drops.leave();
        if scrolled {
            self.created()
                .controls
                .list
                .hwnd()
                .parent()
                .kill_timer(timer::DROP_SCROLL);
        }
    }

    fn choose_effect(
        &self,
        window: Hwnd,
        point: (i32, i32),
        _default: DropEffect,
        _allowed: SourceEffects,
    ) -> Option<DropEffect> {
        let menu = self.drag_in.borrow().as_ref()?.evaluated.as_ref()?.1.menu?;
        let nodes = DropMenu::nodes(self.model.borrow().strings());
        let chosen = win::menu::Menu::track_popup_in_front(
            window,
            &nodes,
            point,
            &menu.states(),
            Some(menu.default_id()),
        );
        DropMenu::effect_of(chosen)
    }

    fn drag_leave(&self) {
        self.created().work.borrow_mut().drops.leave();
        let drag = self.drag_in.take();
        self.end_drag_in(drag.as_ref());
        let mut retired = Retirement::default();
        retired.push(drag, crate::core::work::INLINE_ITEMS + 1);
        self.created().work.borrow_mut().retire(retired);
    }

    fn dropped(&self, effect: DropEffect, data: DropData) -> Option<Rc<Cell<bool>>> {
        let drag = self.drag_in.take()?;
        self.end_drag_in(Some(&drag));
        self.created().work.borrow_mut().drops.leave();
        let held = (|| {
            let target = drag.target.as_ref()?;
            let destination = PathBuf::from(OsString::from_wide(&wide_path(&target.folder)));
            let id = self.accept_transfer(destination.clone(), effect == DropEffect::Move)?;
            let (source, held) = DropSource::pending();
            if data.is_own() {
                held.set(true);
            }
            self.dropping.borrow_mut().insert(
                id,
                Dropping {
                    destination,
                    moves: effect == DropEffect::Move,
                    source,
                },
            );
            self.created()
                .work
                .borrow_mut()
                .drops
                .setup(id, data, effect);
            Some(held)
        })();
        let mut retired = Retirement::default();
        retired.push(drag, crate::core::work::INLINE_ITEMS + 1);
        self.created().work.borrow_mut().retire(retired);
        held
    }

    fn run_until(&self, window: Hwnd, done: &dyn Fn() -> bool) {
        win::window::run_messages_until(window, &|| self.accelerators(), done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::work::{Replacement, Slot};

    #[test]
    fn hover_expiry_preserves_newer_intent_and_stops_replay() {
        let mut slot = Slot::new(Replacement::Cancel);
        let old = slot.submit((), |_, _| ()).unwrap().id;
        slot.submit((), |_, _| ());
        let current = slot.current().unwrap();
        let mut drag = DragIn {
            landed: false,
            preview: Some(current),
            dragged: None,
            evaluation: None,
            evaluated: None,
            target: None,
            over: Some((
                (10, 20),
                DropKeys {
                    ctrl: false,
                    shift: false,
                    alt: false,
                },
                SourceEffects {
                    copy: true,
                    moves: true,
                },
            )),
            scrolling: None,
        };
        assert!(!drag.expire(Expired::Preview(old)));
        assert!(drag.over.is_some());
        assert_eq!(drag.preview, Some(current));
        assert!(drag.expire(Expired::Preview(current)));
        assert!(drag.over.is_none());
        assert!(drag.preview.is_none());
        assert!(!drag.expire(Expired::Preview(current)));
    }
}
