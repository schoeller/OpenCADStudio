//! Minimal Python `ocs` module injected into the shell subprocess.

/// Return the Python source for the placeholder `ocs` module.
///
/// The module is installed in `sys.modules` before the interactive REPL starts
/// so that `import ocs` works and user scripts can call the placeholder APIs.
pub fn ocs_module_source() -> String {
    r#"
import types
import sys

_ocs = types.ModuleType("ocs")

_ocs.active_document = None
_ocs.active_tab = None

def _ocs_print(*args, **kwargs):
    """Placeholder console printer."""
    print("[ocs]", *args, **kwargs)

_ocs.print = _ocs_print

def _not_implemented(name):
    def fn(*args, **kwargs):
        print(f"ocs module: {name} is not yet implemented")
    return fn

_ocs.add_point = _not_implemented("add_point")
_ocs.add_line = _not_implemented("add_line")
_ocs.push_undo = _not_implemented("push_undo")
_ocs.refresh_document = _not_implemented("refresh_document")

sys.modules["ocs"] = _ocs
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_source_contains_expected_placeholders() {
        let src = ocs_module_source();
        assert!(src.contains("active_document"));
        assert!(src.contains("active_tab"));
        assert!(src.contains("def _ocs_print"));
        assert!(src.contains("sys.modules[\"ocs\"]"));
    }
}
