import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useCursorPageStack } from "./useCursorPageStack";

describe("useCursorPageStack", () => {
  it("starts at page 0 with null cursor and zero itemsBefore", () => {
    const { result } = renderHook(() => useCursorPageStack());
    expect(result.current.cursor).toBeNull();
    expect(result.current.pageIndex).toBe(0);
    expect(result.current.hasPrev).toBe(false);
    expect(result.current.itemsBefore).toBe(0);
  });

  it("goNext pushes cursor and advances page index", () => {
    const { result } = renderHook(() => useCursorPageStack());
    act(() => result.current.goNext("cursor-1", 25));
    expect(result.current.cursor).toBe("cursor-1");
    expect(result.current.pageIndex).toBe(1);
    expect(result.current.hasPrev).toBe(true);
    expect(result.current.itemsBefore).toBe(25);
  });

  it("goNext ignores null cursor", () => {
    const { result } = renderHook(() => useCursorPageStack());
    act(() => result.current.goNext(null, 25));
    expect(result.current.cursor).toBeNull();
    expect(result.current.pageIndex).toBe(0);
    expect(result.current.itemsBefore).toBe(0);
  });

  it("itemsBefore accumulates across pages with non-uniform sizes", () => {
    const { result } = renderHook(() => useCursorPageStack());
    // Page 0: 25 items → page 1
    act(() => result.current.goNext("c1", 25));
    expect(result.current.itemsBefore).toBe(25);
    // Page 1: 17 items (short page) → page 2
    act(() => result.current.goNext("c2", 17));
    expect(result.current.itemsBefore).toBe(42);
    // Page 2: 3 items (last page)
    act(() => result.current.goNext("c3", 3));
    expect(result.current.itemsBefore).toBe(45);
  });

  it("goPrev returns to previous page with correct itemsBefore", () => {
    const { result } = renderHook(() => useCursorPageStack());
    act(() => result.current.goNext("cursor-1", 25));
    act(() => result.current.goNext("cursor-2", 17));
    expect(result.current.cursor).toBe("cursor-2");
    expect(result.current.pageIndex).toBe(2);
    expect(result.current.itemsBefore).toBe(42);

    act(() => result.current.goPrev());
    expect(result.current.cursor).toBe("cursor-1");
    expect(result.current.pageIndex).toBe(1);
    expect(result.current.itemsBefore).toBe(25);
    expect(result.current.hasPrev).toBe(true);
  });

  it("goPrev is no-op when on first page", () => {
    const { result } = renderHook(() => useCursorPageStack());
    act(() => result.current.goPrev());
    expect(result.current.cursor).toBeNull();
    expect(result.current.pageIndex).toBe(0);
    expect(result.current.itemsBefore).toBe(0);
    expect(result.current.hasPrev).toBe(false);
  });

  it("reset clears stack to first page", () => {
    const { result } = renderHook(() => useCursorPageStack());
    act(() => result.current.goNext("cursor-1", 25));
    act(() => result.current.goNext("cursor-2", 17));
    act(() => result.current.reset());
    expect(result.current.cursor).toBeNull();
    expect(result.current.pageIndex).toBe(0);
    expect(result.current.itemsBefore).toBe(0);
    expect(result.current.hasPrev).toBe(false);
  });

  it("multiple goPrev/goNext cycles maintain correct state", () => {
    const { result } = renderHook(() => useCursorPageStack());
    act(() => result.current.goNext("c1", 25));
    act(() => result.current.goNext("c2", 17));
    act(() => result.current.goPrev());
    act(() => result.current.goNext("c2", 17));
    expect(result.current.cursor).toBe("c2");
    expect(result.current.pageIndex).toBe(2);
    expect(result.current.itemsBefore).toBe(42);
    expect(result.current.hasPrev).toBe(true);
  });

  it("goNext after going back replaces forward history", () => {
    const { result } = renderHook(() => useCursorPageStack());
    act(() => result.current.goNext("c1", 25));
    act(() => result.current.goNext("c2", 17));
    // Go back to page 1
    act(() => result.current.goPrev());
    // Go forward to page 2 with a new cursor
    act(() => result.current.goNext("c2b", 17));
    expect(result.current.cursor).toBe("c2b");
    expect(result.current.pageIndex).toBe(2);
    expect(result.current.itemsBefore).toBe(42);
  });
});
