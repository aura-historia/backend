import { L as LabColor, X as XyzColor, I as XyzD65Color, A as AnyColor, s as Plugin } from '../colordx-JjvrmuHd.cjs';

declare module '@colordx/core' {
    interface Colordx {
        toLab(precision?: number): LabColor;
        toLabString(precision?: number): string;
        toXyz(precision?: number): XyzColor;
        toXyzString(precision?: number): string;
        toXyzD65(precision?: number): XyzD65Color;
        toXyzD65String(precision?: number): string;
        mixLab(color: AnyColor, ratio?: number): Colordx;
        delta(color?: AnyColor): number;
    }
}
declare const lab: Plugin;

export { lab as default };
