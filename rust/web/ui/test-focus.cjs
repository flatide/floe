'use strict';
// Shared focus DTO/ACK fixture, not an application fallback. Held/error/cancel
// outcomes are exercised separately by the isolation and actual-client tests.
const assert = require('node:assert/strict');
const token = 'a'.repeat(64);
exports.reply = (q, value) => ({check:q.check,local:q.error,prepared_token:token,
    layer_isolation:{status:'no_metadata',matched:'0'},...value});
exports.accept = moves => (navigation, prepared, done) => {
    assert.equal(prepared, token); moves.push(navigation); done(null); return () => false;
};
