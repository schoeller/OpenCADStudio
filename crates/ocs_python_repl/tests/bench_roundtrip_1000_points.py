import ocs, random, time

def roundtrip_1000_points():
    random.seed(42)
    pts = [(random.uniform(0, 1000), random.uniform(0, 1000), 0.0) for _ in range(1000)]

    t0 = time.perf_counter()
    ocs.doc.add_many(ocs.Point(x, y, z, layer="PTS") for x, y, z in pts)
    ocs.doc.commit()
    t1 = time.perf_counter()
    add_time = t1 - t0

    assert len(ocs.doc.entities) == 1000, f"expected 1000 points, got {len(ocs.doc.entities)}"

    t0 = time.perf_counter()
    ocs.doc.remove_all()
    ocs.doc.commit()
    t1 = time.perf_counter()
    remove_time = t1 - t0

    assert len(ocs.doc.entities) == 0, f"expected 0 points, got {len(ocs.doc.entities)}"
    return add_time, remove_time

if __name__ == "__main__":
    add_time, remove_time = roundtrip_1000_points()
    print(f"add 1000 points: {add_time:.3f}s")
    print(f"remove 1000 points: {remove_time:.3f}s")
    assert add_time + remove_time < 1.0, "roundtrip too slow"
