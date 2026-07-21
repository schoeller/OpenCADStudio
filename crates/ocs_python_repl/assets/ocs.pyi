# Type stubs for the ocs extension (ocs_acadifc aliased to ocs at runtime).
# This file is only for static analysis; the actual implementation is the
# compiled PyO3 extension loaded by the REPL bootstrap.

from typing import Iterable, Iterator, List, Optional, Tuple, Union

class Vector3:
    x: float
    y: float
    z: float
    def __init__(self, x: float = 0.0, y: float = 0.0, z: float = 0.0) -> None: ...
    def __repr__(self) -> str: ...

class Color:
    r: int
    g: int
    b: int
    def __init__(self, r: int = 0, g: int = 0, b: int = 0) -> None: ...

class Layer:
    name: str

class Entity:
    handle: int
    kind: str
    layer: str
    def delete(self) -> None: ...

class Point(Entity):
    x: float
    y: float
    z: float
    point: Vector3
    def __init__(
        self, x: float = 0.0, y: float = 0.0, z: float = 0.0, layer: Optional[str] = None
    ) -> None: ...

class Line(Entity):
    start: Vector3
    end: Vector3
    def __init__(
        self,
        start: Optional[Vector3] = None,
        end: Optional[Vector3] = None,
        layer: Optional[str] = None,
    ) -> None: ...

class Circle(Entity):
    center: Vector3
    radius: float
    def __init__(
        self,
        center: Optional[Vector3] = None,
        radius: float = 1.0,
        layer: Optional[str] = None,
    ) -> None: ...

class Arc(Entity):
    center: Vector3
    radius: float
    start_angle: float
    end_angle: float
    def __init__(
        self,
        center: Optional[Vector3] = None,
        radius: float = 1.0,
        start_angle: float = 0.0,
        end_angle: float = 1.57079632679,
        layer: Optional[str] = None,
    ) -> None: ...

class Text(Entity):
    value: str
    location: Vector3
    height: float
    layer: str
    def __init__(
        self,
        value: Optional[str] = None,
        x: float = 0.0,
        y: float = 0.0,
        z: float = 0.0,
        height: float = 2.5,
        layer: Optional[str] = None,
    ) -> None: ...

class MText(Entity):
    value: str
    insertion_point: Vector3
    height: float
    layer: str
    def __init__(
        self,
        value: Optional[str] = None,
        x: float = 0.0,
        y: float = 0.0,
        z: float = 0.0,
        height: float = 2.5,
        layer: Optional[str] = None,
    ) -> None: ...

class LwPolyline(Entity):
    vertices: List[Tuple[float, float, float]]
    is_closed: bool
    layer: str
    def __init__(
        self,
        vertices: Optional[List[Tuple[float, float, float]]] = None,
        is_closed: bool = False,
        layer: Optional[str] = None,
    ) -> None: ...

class Insert(Entity):
    block_name: str
    insertion_point: Vector3
    rotation: float
    layer: str
    def __init__(
        self,
        block_name: str,
        x: float = 0.0,
        y: float = 0.0,
        z: float = 0.0,
        rotation: float = 0.0,
        layer: Optional[str] = None,
    ) -> None: ...


class Document:
    version: int
    layers: List[Layer]
    text_styles: List[str]
    dim_styles: List[str]
    blocks: List[str]
    @property
    def entities(self) -> List[Entity]: ...
    def add(self, entity: Entity) -> None: ...
    def add_many(self, entities: Iterable[Entity]) -> None: ...
    def remove_all(self) -> None: ...
    def commit(self) -> None: ...
    def refresh(self) -> None: ...

class PyMutationQueue: ...

doc: Document

__version__: str

def _init(snapshot_path: str, queue_path: str, control_socket: str) -> None: ...
def get_doc() -> Document: ...
def make_point(
    x: float = 0.0, y: float = 0.0, z: float = 0.0, layer: Optional[str] = None
) -> Point: ...
def make_line(
    start: Vector3, end: Vector3, layer: Optional[str] = None
) -> Line: ...
def make_circle(
    center: Vector3, radius: float = 1.0, layer: Optional[str] = None
) -> Circle: ...
def make_arc(
    center: Vector3,
    radius: float = 1.0,
    start_angle: float = 0.0,
    end_angle: float = 1.57079632679,
    layer: Optional[str] = None,
) -> Arc: ...
def make_text(
    value: Optional[str] = None,
    x: float = 0.0,
    y: float = 0.0,
    z: float = 0.0,
    height: float = 2.5,
    layer: Optional[str] = None,
) -> Text: ...
def make_mtext(
    value: Optional[str] = None,
    x: float = 0.0,
    y: float = 0.0,
    z: float = 0.0,
    height: float = 2.5,
    layer: Optional[str] = None,
) -> MText: ...
def make_lwpolyline(
    vertices: Optional[List[Tuple[float, float, float]]] = None,
    is_closed: bool = False,
    layer: Optional[str] = None,
) -> LwPolyline: ...
def make_insert(
    block_name: str,
    x: float = 0.0,
    y: float = 0.0,
    z: float = 0.0,
    rotation: float = 0.0,
    layer: Optional[str] = None,
) -> Insert: ...
