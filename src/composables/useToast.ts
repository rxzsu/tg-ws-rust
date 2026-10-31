import { ref } from "vue";

export interface ToastItem {
  id: number;
  kind: "ok" | "error" | "info";
  message: string;
}

const toasts = ref<ToastItem[]>([]);
let seq = 0;

export function dismissToast(id: number) {
  toasts.value = toasts.value.filter((t) => t.id !== id);
}

export function pushToast(
  message: string,
  kind: ToastItem["kind"] = "ok",
  timeout = 2600,
) {
  const id = ++seq;
  toasts.value.push({ id, kind, message });
  if (toasts.value.length > 4) {
    toasts.value.shift();
  }
  window.setTimeout(() => dismissToast(id), timeout);
}

export function useToasts() {
  return toasts;
}
