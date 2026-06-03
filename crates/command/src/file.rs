//! File interaction commands (`file.open`, `file.open_folder`, `file.save`).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::{CommandId, ContextPredicate, Error, Registry};

/// Open/import one or more files.
pub const FILE_OPEN: CommandId = CommandId("file.open");
/// Open a folder in the left file-tree pane.
pub const FILE_OPEN_FOLDER: CommandId = CommandId("file.open_folder");
/// Save the active buffer back to its associated file.
pub const FILE_SAVE: CommandId = CommandId("file.save");
/// Save the active buffer to a chosen path and associate it.
pub const FILE_SAVE_AS: CommandId = CommandId("file.save_as");
/// Reload a changed file from disk.
pub const FILE_RELOAD_EXTERNAL: CommandId = CommandId("file.reload_external");
/// Dismiss the external-change prompt and keep editor content.
pub const FILE_KEEP_MINE: CommandId = CommandId("file.keep_mine");
/// Show a diff for the external-change prompt.
pub const FILE_SHOW_DIFF: CommandId = CommandId("file.show_diff");

/// Register Phase-15 file commands.
pub fn register_file_commands(reg: &mut Registry) {
    reg.register(
        FILE_OPEN,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|args, ctx| {
            let paths = parse_paths_arg(args)?;
            let Some(file_context) = ctx.file_context() else {
                return Err(Error::UnsupportedContext("file_open"));
            };
            match paths {
                Some(paths) => file_context.file_open_paths(paths),
                None => file_context.file_open_dialog(),
            }
        }),
    );
    reg.register(
        FILE_OPEN_FOLDER,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|args, ctx| {
            let path = parse_optional_path_arg(args)?;
            let Some(file_context) = ctx.file_context() else {
                return Err(Error::UnsupportedContext("file_open_folder"));
            };
            file_context.file_open_folder(path)
        }),
    );
    reg.register(
        FILE_SAVE,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|_, ctx| {
            let Some(file_context) = ctx.file_context() else {
                return Err(Error::UnsupportedContext("file_save"));
            };
            file_context.file_save()
        }),
    );
    reg.register(
        FILE_SAVE_AS,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|_, ctx| {
            let Some(file_context) = ctx.file_context() else {
                return Err(Error::UnsupportedContext("file_save_as"));
            };
            file_context.file_save_as()
        }),
    );
    reg.register(
        FILE_RELOAD_EXTERNAL,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|_, ctx| {
            let Some(file_context) = ctx.file_context() else {
                return Err(Error::UnsupportedContext("file_reload_external"));
            };
            file_context.file_reload_external()
        }),
    );
    reg.register(
        FILE_KEEP_MINE,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|_, ctx| {
            let Some(file_context) = ctx.file_context() else {
                return Err(Error::UnsupportedContext("file_keep_mine"));
            };
            file_context.file_keep_mine()
        }),
    );
    reg.register(
        FILE_SHOW_DIFF,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|_, ctx| {
            let Some(file_context) = ctx.file_context() else {
                return Err(Error::UnsupportedContext("file_show_diff"));
            };
            file_context.file_show_diff()
        }),
    );
}

fn parse_optional_path_arg(args: &Value) -> Result<Option<PathBuf>, Error> {
    match args {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(PathBuf::from(s))),
        other => Err(Error::InvalidArgs {
            name: FILE_OPEN_FOLDER.as_str(),
            reason: format!("expected string path or null; got {other}"),
        }),
    }
}

fn parse_paths_arg(args: &Value) -> Result<Option<Vec<PathBuf>>, Error> {
    match args {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(vec![PathBuf::from(s)])),
        Value::Array(values) => {
            let mut paths = Vec::with_capacity(values.len());
            for value in values {
                let Some(s) = value.as_str() else {
                    return Err(Error::InvalidArgs {
                        name: FILE_OPEN.as_str(),
                        reason: "array entries must be strings".into(),
                    });
                };
                paths.push(PathBuf::from(s));
            }
            Ok(Some(paths))
        }
        other => Err(Error::InvalidArgs {
            name: FILE_OPEN.as_str(),
            reason: format!("expected string path, string array, or null; got {other}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, FileContext, ViewContext};

    #[derive(Default)]
    struct Ctx {
        opened: Vec<PathBuf>,
        dialog: bool,
        saved: bool,
        saved_as: bool,
    }

    impl ViewContext for Ctx {}

    impl FileContext for Ctx {
        fn file_open_dialog(&mut self) -> Result<(), Error> {
            self.dialog = true;
            Ok(())
        }

        fn file_open_paths(&mut self, paths: Vec<PathBuf>) -> Result<(), Error> {
            self.opened = paths;
            Ok(())
        }

        fn file_save(&mut self) -> Result<(), Error> {
            self.saved = true;
            Ok(())
        }

        fn file_save_as(&mut self) -> Result<(), Error> {
            self.saved_as = true;
            Ok(())
        }

        fn file_reload_external(&mut self) -> Result<(), Error> {
            Ok(())
        }

        fn file_keep_mine(&mut self) -> Result<(), Error> {
            Ok(())
        }

        fn file_show_diff(&mut self) -> Result<(), Error> {
            Ok(())
        }
    }

    impl crate::FindContext for Ctx {}
    impl crate::EditConfigContext for Ctx {}
    impl Context for Ctx {
        fn lookup(&self, key: &str) -> Option<&str> {
            (key == "editor.focused").then_some("true")
        }

        fn file_context(&mut self) -> Option<&mut dyn FileContext> {
            Some(self)
        }
    }

    #[test]
    fn open_dispatches_path_arg() {
        let mut reg = Registry::new();
        register_file_commands(&mut reg);
        let mut ctx = Ctx::default();
        reg.dispatch(FILE_OPEN, &Value::String("a.md".into()), &mut ctx)
            .unwrap();
        assert_eq!(ctx.opened, vec![PathBuf::from("a.md")]);
    }

    #[test]
    fn save_commands_dispatch() {
        let mut reg = Registry::new();
        register_file_commands(&mut reg);
        let mut ctx = Ctx::default();
        reg.dispatch(FILE_SAVE, &Value::Null, &mut ctx).unwrap();
        reg.dispatch(FILE_SAVE_AS, &Value::Null, &mut ctx).unwrap();
        assert!(ctx.saved);
        assert!(ctx.saved_as);
    }
}
