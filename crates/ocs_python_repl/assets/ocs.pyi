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
    styles: dict[str, List[str]]
    @property
    def entities(self) -> List[Entity]: ...
    def add(self, entity: Entity) -> None: ...
    def add_many(self, entities: Iterable[Entity]) -> None: ...
    def add_points(self, points: List[Tuple[float, float, float]], layer: Optional[str] = None) -> None: ...
    def add_lines(
        self, lines: List[Tuple[Tuple[float, float, float], Tuple[float, float, float]]], layer: Optional[str] = None
    ) -> None: ...
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

class Hatch(Entity):
    boundary: List[Tuple[float, float]]
    is_solid: bool
    layer: str
    def __init__(
        self,
        boundary: Optional[List[Tuple[float, float]]] = None,
        is_solid: bool = True,
        layer: Optional[str] = None,
    ) -> None: ...

class Dimension(Entity):
    start: Vector3
    end: Vector3
    layer: str
    def __init__(
        self,
        start: Optional[Vector3] = None,
        end: Optional[Vector3] = None,
        layer: Optional[str] = None,
    ) -> None: ...

class Leader(Entity):
    vertices: List[Vector3]
    layer: str
    def __init__(
        self,
        vertices: Optional[List[Vector3]] = None,
        layer: Optional[str] = None,
    ) -> None: ...

class Viewport(Entity):
    center: Vector3
    width: float
    height: float
    id: int
    layer: str
    def __init__(
        self,
        center: Optional[Vector3] = None,
        width: float = 1.0,
        height: float = 1.0,
        id: int = 2,
        layer: Optional[str] = None,
    ) -> None: ...

class Spline(Entity):
    control_points: List[Vector3]
    knots: List[float]
    degree: int
    is_closed: bool
    layer: str
    def __init__(
        self,
        control_points: Optional[List[Vector3]] = None,
        knots: Optional[List[float]] = None,
        degree: int = 3,
        is_closed: bool = False,
        layer: Optional[str] = None,
    ) -> None: ...

def make_hatch(
    boundary: Optional[List[Tuple[float, float]]] = None,
    is_solid: bool = True,
    layer: Optional[str] = None,
) -> Hatch: ...
def make_dimension(
    start: Optional[Vector3] = None,
    end: Optional[Vector3] = None,
    layer: Optional[str] = None,
) -> Dimension: ...
def make_leader(
    vertices: Optional[List[Vector3]] = None,
    layer: Optional[str] = None,
) -> Leader: ...
def make_viewport(
    center: Optional[Vector3] = None,
    width: float = 1.0,
    height: float = 1.0,
    id: int = 2,
    layer: Optional[str] = None,
) -> Viewport: ...
def make_spline(
    control_points: Optional[List[Vector3]] = None,
    knots: Optional[List[float]] = None,
    degree: int = 3,
    is_closed: bool = False,
    layer: Optional[str] = None,
) -> Spline: ...

class Debug:
    @staticmethod
    def start(port: int = 5678) -> None: ...
    @staticmethod
    def wait_for_client() -> None: ...

debug: Debug
