import { S as SrgbLinearColor, s as Plugin } from '../colordx-JjvrmuHd.js';

declare module '@colordx/core' {
    interface Colordx {
        toSrgbLinear(precision?: number): SrgbLinearColor;
        toSrgbLinearString(precision?: number): string;
    }
}
declare const srgbLinear: Plugin;

export { srgbLinear as default };
