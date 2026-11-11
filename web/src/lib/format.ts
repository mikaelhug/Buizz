export const ABSENT = "—";

const money = new Intl.NumberFormat("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const price = new Intl.NumberFormat("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 4 });
const ratio = new Intl.NumberFormat("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const percent = new Intl.NumberFormat("en-US", {
  style: "percent",
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});
const signedPercent = new Intl.NumberFormat("en-US", {
  style: "percent",
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
  signDisplay: "exceptZero",
});
const integer = new Intl.NumberFormat("en-US");
const dateTime = new Intl.DateTimeFormat("sv-SE", {
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
});

type Maybe<T> = T | null | undefined;

function present<T>(value: Maybe<T>, render: (value: T) => string): string {
  return value === null || value === undefined ? ABSENT : render(value);
}

export const fmt = {
  money: (value: Maybe<string | number>) => present(value, (v) => money.format(Number(v))),
  price: (value: Maybe<string | number>) => present(value, (v) => price.format(Number(v))),
  ratio: (value: Maybe<number>) => present(value, (v) => ratio.format(v)),
  percent: (value: Maybe<string | number>) => present(value, (v) => percent.format(Number(v))),
  change: (value: Maybe<number>) => present(value, (v) => signedPercent.format(v)),
  integer: (value: Maybe<number>) => present(value, (v) => integer.format(v)),
  dateTime: (value: Maybe<string>) => present(value, (v) => dateTime.format(new Date(v))),
};

export function tone(value: Maybe<number>): "up" | "down" | undefined {
  if (value === null || value === undefined || value === 0) {
    return undefined;
  }
  return value > 0 ? "up" : "down";
}

export function label(value: string): string {
  return value.replaceAll("_", " ");
}
