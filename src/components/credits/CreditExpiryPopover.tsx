import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Clock } from "lucide-react";
import type { CreditPack } from "@/lib/api/credits";
import { modelInfoPosition } from "./ModelInfoPopover";

const DAY_MS = 86_400_000;

export interface ExpiryEntry {
  pack: CreditPack;
  /** Whole days until expiry; zero on the expiry day, negative once expired. */
  daysLeft: number;
  expired: boolean;
}

/** Packs with a known expiry, soonest first. */
export function expiryEntries(packs: CreditPack[], now: number): ExpiryEntry[] {
  return packs
    .filter((pack) => pack.expiresAt != null)
    // A fully consumed pack has nothing left to expire, so it never counts as
    // the soonest expiry. An unknown balance stays visible.
    .filter((pack) => pack.remaining == null || pack.remaining > 0)
    .map((pack) => {
      const expiresAt = pack.expiresAt as number;
      return {
        pack,
        daysLeft: Math.ceil((expiresAt - now) / DAY_MS),
        expired: expiresAt <= now,
      };
    })
    .sort((a, b) => (a.pack.expiresAt as number) - (b.pack.expiresAt as number));
}

export const expiryDate = (value: number) =>
  new Date(value).toLocaleString("zh-CN", {
    year: "numeric",
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });

export const expiryCountdown = (entry: ExpiryEntry) =>
  entry.expired ? "已过期" : entry.daysLeft <= 0 ? "今天到期" : `还有 ${entry.daysLeft} 天`;

/** Short summary for the trigger, e.g. "最早 12月10日 到期（还有 30 天）". */
export function expirySummary(entries: ExpiryEntry[]): string {
  if (!entries.length) return "暂无到期信息";
  const soonest = entries[0];
  return `最早 ${expiryDate(soonest.pack.expiresAt as number)} 到期（${expiryCountdown(soonest)}）`;
}

export function CreditExpiryContent({
  entries,
  remaining,
}: {
  entries: ExpiryEntry[];
  remaining: (pack: CreditPack) => string;
}) {
  return (
    <>
      <div className="flex items-center gap-2 text-[10px] font-medium text-slate-500 dark:text-slate-400">
        <Clock size={12} aria-hidden="true" />
        <span>积分包到期时间</span>
      </div>
      {entries.length ? (
        <ul className="mt-3 space-y-2.5">
          {entries.map((entry) => {
            const soon = !entry.expired && entry.daysLeft <= 7;
            return (
              <li
                key={entry.pack.id}
                className="rounded-lg border border-slate-500/10 px-2.5 py-2 text-[11px]"
              >
                <div className="flex items-start justify-between gap-2">
                  <span className="min-w-0 flex-1 break-words font-medium">
                    {entry.pack.name}
                  </span>
                  <span
                    className={
                      soon
                        ? "shrink-0 text-amber-700 dark:text-amber-300"
                        : "shrink-0 text-slate-500 dark:text-slate-400"
                    }
                  >
                    {expiryCountdown(entry)}
                  </span>
                </div>
                <div className="mt-1 flex flex-wrap items-center justify-between gap-2 text-[10px] text-slate-500 dark:text-slate-400">
                  <span>{remaining(entry.pack)}</span>
                  <span className="tabular-nums">
                    {expiryDate(entry.pack.expiresAt as number)} 到期
                  </span>
                </div>
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="mt-3 text-[11px] leading-relaxed text-slate-500 dark:text-slate-400">
          当前账号没有带到期时间的积分包。
        </p>
      )}
      <p className="mt-3 border-t border-slate-500/10 pt-2 text-[10px] leading-relaxed text-slate-500 dark:text-slate-400">
        到期时间来自账号返回的积分包数据，随刷新更新；刷新账户可获取最新到期时间。
      </p>
    </>
  );
}

interface CreditExpiryPopoverProps {
  anchor: HTMLElement;
  entries: ExpiryEntry[];
  remaining: (pack: CreditPack) => string;
  onDismiss: () => void;
}

export function CreditExpiryPopover({
  anchor,
  entries,
  remaining,
  onDismiss,
}: CreditExpiryPopoverProps) {
  const panel = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<ReturnType<typeof modelInfoPosition> | null>(null);

  useLayoutEffect(() => {
    if (!panel.current) return;
    setPosition(
      modelInfoPosition(
        anchor.getBoundingClientRect(),
        panel.current.getBoundingClientRect(),
        { width: window.innerWidth, height: window.innerHeight },
      ),
    );
  }, [anchor, entries]);

  useEffect(() => {
    const outside = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        !panel.current?.contains(event.target) &&
        !anchor.contains(event.target)
      ) {
        onDismiss();
      }
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onDismiss();
      }
    };
    const scroll = (event: Event) => {
      if (event.target instanceof Node && panel.current?.contains(event.target)) return;
      onDismiss();
    };
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("keydown", escape, true);
    window.addEventListener("scroll", scroll, true);
    window.addEventListener("resize", onDismiss);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("keydown", escape, true);
      window.removeEventListener("scroll", scroll, true);
      window.removeEventListener("resize", onDismiss);
    };
  }, [anchor, onDismiss]);

  if (typeof document === "undefined") return null;
  return createPortal(
    <div
      ref={panel}
      role="dialog"
      aria-label="积分包到期时间"
      className="glass-modal fixed z-[80] w-80 max-w-[calc(100vw-24px)] overflow-y-auto rounded-2xl p-4 text-slate-900 dark:text-slate-100"
      style={position ?? { left: 0, top: 0, maxHeight: "calc(100vh - 24px)", visibility: "hidden" }}
    >
      <CreditExpiryContent entries={entries} remaining={remaining} />
    </div>,
    document.body,
  );
}
