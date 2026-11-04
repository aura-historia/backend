import { c as CmykColor, s as Plugin } from '../colordx-JjvrmuHd.js';

declare module '@colordx/core' {
    interface Colordx {
        toCmyk(precision?: number): CmykColor;
        toCmykString(precision?: number): string;
    }
}
declare const cmyk: Plugin;

export { cmyk as default };
