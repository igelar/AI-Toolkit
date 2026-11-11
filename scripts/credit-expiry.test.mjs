import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import { Children, createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";

const require = createRequire(import.meta.url);
const DAY = 86_400_000;
const NOW = Date.UTC(2026, 10, 10, 0, 0, 0);

function load(path, stubs = {}, extra = "") {
  const source = readFileSync(new URL(path, import.meta.url), "utf8");
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2020 },
  });
  const module = { exports: {} };
  runInNewContext(outputText + extra, {
    require: (id) => stubs[id] ?? require(id),
    module,
    exports: module.exports,
  });
  return module.exports;
}

const expiryModule = load("../src/components/credits/CreditExpiryPopover.tsx", {
  "./ModelInfoPopover": {
    modelInfoPosition: () => ({ left: 0, top: 0, maxHeight: 0 }),
  },
  "react-dom": { createPortal: (node) => node },
});
const { expiryEntries, expirySummary, expiryCountdown, expiryDate, CreditExpiryContent } = expiryModule;

const pack = (id, name, expiresAt, remaining = 100) => ({
  id,
  name,
  unit: "credits",
  remaining,
  expiresAt,
});

test("只保留带到期时间的积分包，并按到期时间从近到远排序", () => {
  const entries = expiryEntries(
    [
      pack("late", "购买积分", NOW + 300 * DAY, 2000),
      pack("soon", "签到赠送积分", NOW + 5 * DAY, 400),
      pack("unknown", "无到期积分", null),
    ],
    NOW,
  );
  assert.deepEqual(entries.map((entry) => entry.pack.id), ["soon", "late"]);
  assert.equal(entries[0].daysLeft, 5);
  assert.equal(entries[0].expired, false);
  assert.equal(entries[1].daysLeft, 300);
});

test("到期当天与已过期分别显示，不把过期包当成即将过期", () => {
  const today = expiryEntries([pack("today", "今天到期", NOW + 1000)], NOW)[0];
  assert.equal(today.daysLeft, 1);
  assert.equal(today.expired, false);

  const expired = expiryEntries([pack("gone", "已过期", NOW - DAY)], NOW)[0];
  assert.equal(expired.expired, true);
  assert.equal(expiryCountdown(expired), "已过期");

  const justExpired = expiryEntries([pack("just", "刚过期", NOW)], NOW)[0];
  assert.equal(justExpired.expired, true);
  assert.equal(expiryCountdown(justExpired), "已过期");
});

test("摘要显示最早到期的积分包，没有到期数据时保持未知", () => {
  const entries = expiryEntries([pack("soon", "签到赠送积分", NOW + 12 * DAY)], NOW);
  assert.match(expirySummary(entries), /最早/);
  assert.match(expirySummary(entries), /还有 12 天/);
  assert.equal(expirySummary([]), "暂无到期信息");
});

test("已用完的积分包不参与最早到期，剩余未知的包仍保留", () => {
  const entries = expiryEntries(
    [
      pack("used", "已用完积分", NOW + 2 * DAY, 0),
      pack("live", "有余额积分", NOW + 20 * DAY, 400),
      { id: "unknown-balance", name: "余额未知积分", unit: "credits", remaining: null, expiresAt: NOW + 9 * DAY },
    ],
    NOW,
  );
  assert.deepEqual(entries.map((entry) => entry.pack.id), ["unknown-balance", "live"]);
  // The consumed pack must not become the soonest expiry.
  assert.match(expirySummary(entries), /还有 9 天/);
  assert.doesNotMatch(expirySummary(entries), /还有 2 天/);
});

// The account card renders the section and opens the popover without a request.
const stubs = {
  sonner: { toast: Object.assign(() => {}, { error() {}, success() {} }) },
  "./ModelInfoPopover": { ModelInfoPopover: () => null, modelInfoPosition: () => ({ left: 0, top: 0, maxHeight: 0 }) },
  "./CreditExpiryPopover": expiryModule,
  "@tauri-apps/plugin-shell": { open: async () => {} },
  "@/components/ui/Modal": { Modal: () => null },
  "@/components/ui/AgentIcon": { AgentIcon: () => createElement("span") },
  "@/components/ui/Pressable": {
    Pressable: ({ children, ...props }) => {
      const label = Children.toArray(children).filter((child) => typeof child === "string").join("");
      state.buttons.push({ label, ...props });
      return createElement("button", props, children);
    },
  },
  "@/hooks/useCredits": {
    useCreditsAction: (action) => ({
      isPending: false,
      mutate: (input, options) => {
        state.pending.push(Promise.resolve().then(() => action(input)).then((value) => options?.onSuccess?.(value)));
      },
    }),
  },
  "@/lib/api/credits": {
    creditsApi: {
      refresh: async () => [],
      claim: async (id) => { state.claims.push(id); return []; },
      setAccountEnabled: async () => {},
    },
  },
};

let state;
const { AccountCard } = load(
  "../src/components/credits/CreditsPanel.tsx",
  stubs,
  "\nmodule.exports.AccountCard = AccountCard;",
);

function renderAccount(overrides = {}) {
  state = { buttons: [], pending: [], claims: [] };
  const account = {
    id: "test-account",
    platform: "workbuddy",
    region: "cn",
    userId: "test-user",
    label: "测试账户",
    enabled: true,
    status: "ready",
    lastError: null,
    snapshot: {
      balance: 100,
      unit: "credits",
      quota: null,
      packs: [],
      rewards: [],
      models: [],
      fetchedAt: NOW,
      stale: false,
      diagnostic: null,
    },
    ...overrides,
  };
  return renderToStaticMarkup(createElement(AccountCard, { account, onDelete() {} }));
}

test("国内版账户显示即将过期区块，读取已同步的积分包数据", () => {
  const expiresAt = NOW + 12 * DAY;
  const html = renderAccount({
    snapshot: {
      balance: 100,
      unit: "credits",
      quota: null,
      packs: [pack("soon", "签到赠送积分", expiresAt, 400)],
      rewards: [],
      models: [],
      fetchedAt: NOW,
      stale: false,
      diagnostic: null,
    },
  });
  assert.match(html, /即将过期/);
  assert.match(html, /最早/);
  // The card counts down against the wall clock, so the exact day count is
  // asserted by the pure-function tests; here only the rendered date matters.
  assert.ok(html.includes(expiryDate(expiresAt)), "未显示积分包到期日期");
  assert.match(html, /到期（还有 \d+ 天）/);
  // No new request is made when the section is shown.
  assert.equal(state.claims.length, 0);
});

test("没有到期数据的账户仍然显示区块，并保持未知", () => {
  const html = renderAccount();
  assert.match(html, /即将过期/);
  assert.match(html, /暂无到期信息/);
});

test("点击即将过期区块打开到期时间浮层，列出每个积分包", () => {
  renderAccount({
    snapshot: {
      balance: 100,
      unit: "credits",
      quota: null,
      packs: [
        pack("soon", "签到赠送积分", NOW + 12 * DAY, 400),
        pack("late", "购买积分", NOW + 300 * DAY, 2000),
      ],
      rewards: [],
      models: [],
      fetchedAt: NOW,
      stale: false,
      diagnostic: null,
    },
  });
  const trigger = state.buttons.find((button) => button["aria-label"] === "查看积分包到期时间");
  assert.ok(trigger, "即将过期触发入口缺失");
  trigger.onClick({ currentTarget: {} });
  const html = renderToStaticMarkup(
    createElement(CreditExpiryContent, {
      entries: expiryEntries(
        [
          pack("soon", "签到赠送积分", NOW + 12 * DAY, 400),
          pack("late", "购买积分", NOW + 300 * DAY, 2000),
        ],
        NOW,
      ),
      remaining: (p) => `剩余 ${p.remaining} 积分`,
    }),
  );
  assert.match(html, /签到赠送积分/);
  assert.match(html, /购买积分/);
  assert.match(html, /剩余 400 积分/);
  assert.match(html, /还有 12 天/);
  assert.match(html, /还有 300 天/);
});

test("国际版账户保留额度明细，不显示即将过期区块", () => {
  const html = renderAccount({
    region: "intl",
    snapshot: {
      balance: 100,
      unit: "credits",
      quota: null,
      packs: [pack("soon", "签到赠送积分", NOW + 12 * DAY, 400)],
      rewards: [],
      models: [],
      fetchedAt: NOW,
      stale: false,
      diagnostic: null,
    },
  });
  assert.match(html, /额度明细/);
  assert.match(html, /签到赠送积分/);
  assert.doesNotMatch(html, /即将过期/);
});
