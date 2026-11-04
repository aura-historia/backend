"use strict";

var _interopRequireDefault = require("@babel/runtime/helpers/interopRequireDefault").default;
Object.defineProperty(exports, "__esModule", {
  value: true
});
exports.default = requiredTitle;
var _ruleError = _interopRequireDefault(require("./ruleError"));
function requiredTitle(element) {
  const {
    tagName,
    children
  } = element;

  // On root mjml, detect missing head/title
  if (tagName === 'mjml') {
    const rootChildren = Array.isArray(children) ? children : [];
    const headEl = rootChildren.find(c => c && c.tagName === 'mj-head');
    if (!headEl) {
      return (0, _ruleError.default)('Missing mj-title. Provide non-empty content for a valid <title>.', element);
    }
    const headChildren = Array.isArray(headEl.children) ? headEl.children : [];
    const titleEl = headChildren.find(c => c && c.tagName === 'mj-title');
    if (!titleEl) {
      return (0, _ruleError.default)('Missing mj-title. Provide non-empty content for a valid <title>.', headEl);
    }
    // If present, the empty case will be handled when visiting mj-head
    return null;
  }

  // On mj-head, warn only when an explicit mj-title is provided but empty/whitespace.
  if (tagName !== 'mj-head') return null;
  const headChildren = Array.isArray(children) ? children : [];
  const titleEl = headChildren.find(c => c && c.tagName === 'mj-title');
  if (!titleEl) return null;
  const content = (titleEl.content || '').trim();
  if (!content) {
    return (0, _ruleError.default)('Empty mj-title. Provide non-empty content for a valid <title>.', titleEl);
  }
  return null;
}
module.exports = exports.default;