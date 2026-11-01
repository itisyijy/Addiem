// 여러 화면이 같이 보는 값 하나. set 하면 use() 로 보는 화면이 다시 그려진다
import { useSyncExternalStore } from "react";

export function store<T>(value: T) {
  const subs = new Set<() => void>();
  const sub = (f: () => void) => (subs.add(f), () => void subs.delete(f));
  const get = () => value;
  return {
    get,
    set: (v: T) => ((value = v), subs.forEach((f) => f())),
    use: () => useSyncExternalStore(sub, get),
  };
}
