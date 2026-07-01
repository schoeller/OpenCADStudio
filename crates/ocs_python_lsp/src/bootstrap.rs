//! Embedded Python bootstrap exposed as an `ocs` module to user scripts.

/// Python bootstrap. It exposes a high-level `ocs` object whose methods issue
/// JSON requests on `stderr` and read matching `__ocs_resp__` replies from
/// `stdin`. Code to execute is delivered as `CODE <base64>` lines on `stdin`.
pub const BOOTSTRAP: &str = r#"
import sys, json, traceback, base64

class OcsError(Exception):
    pass

def _call(req):
    sys.stderr.write(json.dumps(req, separators=(',', ':')) + '\n')
    sys.stderr.flush()
    while True:
        line = sys.stdin.readline()
        if not line:
            raise EOFError('lost connection to OpenCAD Studio host')
        line = line.rstrip('\n').rstrip('\r')
        if line.startswith('__ocs_resp__ '):
            payload = line[len('__ocs_resp__ '):]
            resp = json.loads(payload)
            if resp.get('type') == 'Error':
                raise OcsError(resp.get('value'))
            return resp

class Doc:
    def entities(self):
        return _call({'type': 'GetEntities'})['value']

    def layers(self):
        return _call({'type': 'GetLayers'})['value']

    def layer_name(self, handle):
        return _call({'type': 'LayerName', 'value': handle})['value']

    def app_id_name(self, handle):
        return _call({'type': 'AppIdName', 'value': handle})['value']

class Ocs:
    doc = Doc()

    def push_info(self, msg):
        _call({'type': 'PushInfo', 'value': str(msg)})

    def push_output(self, msg):
        _call({'type': 'PushOutput', 'value': str(msg)})

    def push_error(self, msg):
        _call({'type': 'PushError', 'value': str(msg)})

    def exit(self):
        _call({'type': 'Exit'})
        sys.exit(0)

    def add_point(self, x, y, z=0.0, layer='0'):
        return _call({'type': 'AddPoint', 'value': {'x': x, 'y': y, 'z': z, 'layer': layer}})['value']

    def add_line(self, x1, y1, z1, x2, y2, z2, layer='0'):
        return _call({'type': 'AddLine', 'value': {
            'x1': x1, 'y1': y1, 'z1': z1,
            'x2': x2, 'y2': y2, 'z2': z2,
            'layer': layer}})['value']

    def add_circle(self, x, y, z, radius, layer='0'):
        return _call({'type': 'AddCircle', 'value': {
            'x': x, 'y': y, 'z': z, 'radius': radius, 'layer': layer}})['value']

    def add_text(self, x, y, z, text, height=10.0, layer='0'):
        return _call({'type': 'AddText', 'value': {
            'x': x, 'y': y, 'z': z, 'text': text,
            'height': height, 'layer': layer}})['value']

    def read_xdata(self, handle, app_name):
        return _call({'type': 'ReadRecord', 'value': {
            'handle': handle, 'app_name': app_name}})['value']

    def write_xdata(self, handle, app_name, data):
        record = dict(data)
        record.setdefault('application_name', app_name)
        return _call({'type': 'WriteRecord', 'value': {
            'handle': handle, 'record': record}})['value']

    def remove_xdata(self, handle, app_name):
        return _call({'type': 'RemoveRecord', 'value': {
            'handle': handle, 'app_name': app_name}})['value']

    def bump_geometry(self):
        _call({'type': 'BumpGeometry'})

    def set_dirty(self):
        _call({'type': 'SetDirty'})

    def push_undo(self, label):
        _call({'type': 'PushUndo', 'value': str(label)})

    def counts(self):
        # Local counters; host-side tracking is planned.
        return {'written': 0, 'erased': 0}

    class debug:
        @staticmethod
        def start(port=5678):
            try:
                import debugpy
                debugpy.listen(('127.0.0.1', port))
                debugpy.wait_for_client()
                return {'port': port}
            except Exception as e:
                raise OcsError(f'debugpy not available: {e}')

    def erase(self, handle):
        _call({'type': 'Erase', 'value': handle})

    def erase_by_layer(self, layer):
        _call({'type': 'EraseByLayer', 'value': layer})

    def erase_all(self):
        _call({'type': 'EraseAll', 'value': None})

_ocs = Ocs()
sys.modules['ocs'] = _ocs

print('Python LSP worker ready', flush=True)
_globals = {'ocs': _ocs, '__name__': '__main__'}
_locals = {}
while True:
    line = sys.stdin.readline()
    if not line:
        break
    line = line.rstrip('\n').rstrip('\r')
    if line.startswith('CODE '):
        payload = line[len('CODE '):]
        try:
            code = base64.b64decode(payload).decode('utf-8')
        except Exception:
            print(traceback.format_exc().strip(), flush=True)
        else:
            try:
                try:
                    result = eval(compile(code, '<stdin>', 'eval'), _globals, _locals)
                except SyntaxError:
                    exec(compile(code, '<stdin>', 'exec'), _globals, _locals)
                else:
                    if result is not None:
                        print(repr(result), flush=True)
            except Exception:
                print(traceback.format_exc().strip(), flush=True)
        print('__ocs_done__', flush=True)
"#;
