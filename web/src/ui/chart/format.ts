const CENTS_BELOW = 10_000;
const dollars = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 0 });
const cents = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 2 });
const moneyTicks = [0, 1, 2].map(
  (digits) => new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", minimumFractionDigits: digits, maximumFractionDigits: digits }),
);
const percentTicks = [0, 1, 2].map(
  (digits) => new Intl.NumberFormat("en-US", { style: "percent", minimumFractionDigits: digits, maximumFractionDigits: digits }),
);
const signedPercent = [0, 1, 2].map(
  (digits) => new Intl.NumberFormat("en-US", { minimumFractionDigits: digits, maximumFractionDigits: digits, signDisplay: "exceptZero" }),
);

function stepDigits(values: number[], unit: number): number {
  let step = Number.POSITIVE_INFINITY;
  for (let index = 1; index < values.length; index += 1) {
    step = Math.min(step, Math.abs((values[index] ?? 0) - (values[index - 1] ?? 0)));
  }
  if (!Number.isFinite(step) || step >= unit) {
    return 0;
  }
  return step >= unit / 10 ? 1 : 2;
}

function pick(formats: Intl.NumberFormat[], digits: number): Intl.NumberFormat {
  return formats[digits] ?? formats[formats.length - 1] ?? dollars;
}

export function money(value: number): string {
  return Math.abs(value) < CENTS_BELOW ? cents.format(value) : dollars.format(value);
}

export function moneyTickmarks(values: number[]): string[] {
  const format = pick(moneyTicks, stepDigits(values, 1));
  return values.map((value) => format.format(value));
}

export function ratioTickmarks(values: number[]): string[] {
  const format = pick(percentTicks, stepDigits(values, 0.01));
  return values.map((value) => format.format(value));
}

export function changeTickmarks(values: number[]): string[] {
  const format = pick(signedPercent, stepDigits(values, 1));
  return values.map((value) => `${format.format(value)}%`);
}

export function change(value: number): string {
  return `${pick(signedPercent, 2).format(value)}%`;
}

export function wallDate(time: number, withTime: boolean): string {
  const iso = new Date(time * 1000).toISOString();
  return withTime ? `${iso.slice(0, 10)} ${iso.slice(11, 16)} ET` : iso.slice(0, 10);
}
