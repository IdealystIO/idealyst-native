/* @ts-self-types="./dynlink_mainweb.d.ts" */

function __wbg_reset_state () {
    __wbg_instance_id++;
    cachedBigInt64ArrayMemory0 = null;
    cachedBigUint64ArrayMemory0 = null;
    cachedDataViewMemory0 = null;
    cachedFloat32ArrayMemory0 = null;
    cachedFloat64ArrayMemory0 = null;
    cachedInt16ArrayMemory0 = null;
    cachedInt32ArrayMemory0 = null;
    cachedInt8ArrayMemory0 = null;
    cachedUint16ArrayMemory0 = null;
    cachedUint32ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    if (typeof numBytesDecoded !== 'undefined') numBytesDecoded = 0;
    if (typeof WASM_VECTOR_LEN !== 'undefined') WASM_VECTOR_LEN = 0;
    __wbg_reinit_scheduled = false;
    wasmInstance = new WebAssembly.Instance(wasmModule, __wbg_get_imports());
    wasm = wasmInstance.exports;
    wasm.__wbindgen_start();
}

export function boot() {
    __wbg_call_guard();
    wasm.boot();
}
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg_1_1af35703037e724e: function() {
            const ret = RegExp.$1;
            return ret;
        },
        __wbg_2_85f568a533c31f05: function() {
            const ret = RegExp.$2;
            return ret;
        },
        __wbg_3_defa8d6ccfc916fa: function() {
            const ret = RegExp.$3;
            return ret;
        },
        __wbg_4_8e95e0aba7624b1f: function() {
            const ret = RegExp.$4;
            return ret;
        },
        __wbg_5_e86f774b9abe1f46: function() {
            const ret = RegExp.$5;
            return ret;
        },
        __wbg_6_baae0817744b94fd: function() {
            const ret = RegExp.$6;
            return ret;
        },
        __wbg_7_ac1c977091c3533a: function() {
            const ret = RegExp.$7;
            return ret;
        },
        __wbg_8_10a85695be13c6e5: function() {
            const ret = RegExp.$8;
            return ret;
        },
        __wbg_9_af6dd2274af87a99: function() {
            const ret = RegExp.$9;
            return ret;
        },
        __wbg_BigInt_590a7bb99baad06a: function(arg0) {
            const ret = BigInt(arg0);
            return ret;
        },
        __wbg_BigInt_bead01bd3c1413da: function(arg0, arg1) {
            const ret = BigInt(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_BigInt_d576983233c6e0d1: function() { return handleError(function (arg0) {
            const ret = BigInt(arg0);
            return ret;
        }, arguments); },
        __wbg_Error_ef53bc310eb298a0: function(arg0, arg1) {
            const ret = Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_Number_6b506e6536831eaa: function(arg0) {
            const ret = Number(arg0);
            return ret;
        },
        __wbg_Symbol_991ee0ae3fc4c18f: function(arg0, arg1) {
            const ret = Symbol(arg0 === 0 ? undefined : getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_URL_770be900109120c5: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.URL;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_UTC_1987ef25450ece9c: function(arg0, arg1) {
            const ret = Date.UTC(arg0, arg1);
            return ret;
        },
        __wbg___wbindgen_add_626796fdd5b56298: function(arg0, arg1) {
            const ret = arg0 + arg1;
            return ret;
        },
        __wbg___wbindgen_bigint_get_as_i64_38130e98eecd467d: function(arg0, arg1) {
            const v = arg1;
            const ret = typeof(v) === 'bigint' ? v : undefined;
            getDataViewMemory0().setBigInt64(arg0 + 8 * 1, isLikeNone(ret) ? BigInt(0) : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_bit_and_c3ff9328af026c6b: function(arg0, arg1) {
            const ret = arg0 & arg1;
            return ret;
        },
        __wbg___wbindgen_bit_not_7f45dda564053360: function(arg0) {
            const ret = ~arg0;
            return ret;
        },
        __wbg___wbindgen_bit_or_df5d1a36f9eb4d41: function(arg0, arg1) {
            const ret = arg0 | arg1;
            return ret;
        },
        __wbg___wbindgen_bit_xor_54aaed3586e1108e: function(arg0, arg1) {
            const ret = arg0 ^ arg1;
            return ret;
        },
        __wbg___wbindgen_boolean_get_1a45e2c38d4d41b9: function(arg0) {
            const v = arg0;
            const ret = typeof(v) === 'boolean' ? v : undefined;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg___wbindgen_checked_div_35c40bdd88053ab2: function(arg0, arg1) {
            let result;
            try {
                result = arg0 / arg1;
            } catch (e) {
                if (e instanceof RangeError) {
                    result = e;
                } else {
                    throw e;
                }
            }
            const ret = result;
            return ret;
        },
        __wbg___wbindgen_copy_to_typed_array_7a3f7b938f93cf12: function(arg0, arg1, arg2) {
            new Uint8Array(arg2.buffer, arg2.byteOffset, arg2.byteLength).set(getArrayU8FromWasm0(arg0, arg1));
        },
        __wbg___wbindgen_debug_string_0accd80f45e5faa2: function(arg0, arg1) {
            const ret = debugString(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_div_f13a104a108657d3: function(arg0, arg1) {
            const ret = arg0 / arg1;
            return ret;
        },
        __wbg___wbindgen_exports_24886c422f1ca791: function() {
            const ret = wasm;
            return ret;
        },
        __wbg___wbindgen_function_table_71e3570f9d5d61ae: function() {
            const ret = wasm.__indirect_function_table;
            return ret;
        },
        __wbg___wbindgen_ge_8395c45f619ac11e: function(arg0, arg1) {
            const ret = arg0 >= arg1;
            return ret;
        },
        __wbg___wbindgen_gt_20c8efd5c7a05550: function(arg0, arg1) {
            const ret = arg0 > arg1;
            return ret;
        },
        __wbg___wbindgen_in_70a403a56e771704: function(arg0, arg1) {
            const ret = arg0 in arg1;
            return ret;
        },
        __wbg___wbindgen_instance_d4c1cbc58ce4b84a: function() {
            const ret = wasmInstance;
            return ret;
        },
        __wbg___wbindgen_is_bigint_6ffd6468a9bc44b9: function(arg0) {
            const ret = typeof(arg0) === 'bigint';
            return ret;
        },
        __wbg___wbindgen_is_falsy_d7ed49840e6d9abb: function(arg0) {
            const ret = !arg0;
            return ret;
        },
        __wbg___wbindgen_is_function_754e9f305ff6029e: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_null_87c3bfe968c6a5ad: function(arg0) {
            const ret = arg0 === null;
            return ret;
        },
        __wbg___wbindgen_is_null_or_undefined_cf617b836541fad3: function(arg0) {
            const ret = arg0 == null;
            return ret;
        },
        __wbg___wbindgen_is_object_56732c2bc353f41d: function(arg0) {
            const val = arg0;
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_string_c236cabd84a4d769: function(arg0) {
            const ret = typeof(arg0) === 'string';
            return ret;
        },
        __wbg___wbindgen_is_symbol_7ecbb1d037c25bc3: function(arg0) {
            const ret = typeof(arg0) === 'symbol';
            return ret;
        },
        __wbg___wbindgen_is_undefined_67b456be8673d3d7: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_jsval_eq_1068e624fa87f6ab: function(arg0, arg1) {
            const ret = arg0 === arg1;
            return ret;
        },
        __wbg___wbindgen_jsval_loose_eq_2c56564c75129511: function(arg0, arg1) {
            const ret = arg0 == arg1;
            return ret;
        },
        __wbg___wbindgen_le_9e46e3eac484b44d: function(arg0, arg1) {
            const ret = arg0 <= arg1;
            return ret;
        },
        __wbg___wbindgen_lt_b4bcf1fdfe2e41fe: function(arg0, arg1) {
            const ret = arg0 < arg1;
            return ret;
        },
        __wbg___wbindgen_memory_fbc4c3e30b409f08: function() {
            const ret = wasm.memory;
            return ret;
        },
        __wbg___wbindgen_module_5dcc25d553a4424f: function() {
            const ret = wasmModule;
            return ret;
        },
        __wbg___wbindgen_mul_3bff0ba867bd63d3: function(arg0, arg1) {
            const ret = arg0 * arg1;
            return ret;
        },
        __wbg___wbindgen_neg_c5be7a95a9dd509c: function(arg0) {
            const ret = -arg0;
            return ret;
        },
        __wbg___wbindgen_number_get_9bb1761122181af2: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'number' ? obj : undefined;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_pow_37994975be8c6c55: function(arg0, arg1) {
            const ret = arg0 ** arg1;
            return ret;
        },
        __wbg___wbindgen_reinit_f27b6508badc3263: function() {
            __wbg_reinit_scheduled = true;
        },
        __wbg___wbindgen_rem_a72f52ef022a6366: function(arg0, arg1) {
            const ret = arg0 % arg1;
            return ret;
        },
        __wbg___wbindgen_rethrow_c4d99b4b53265290: function(arg0) {
            throw arg0;
        },
        __wbg___wbindgen_shl_9f0bde039d054e42: function(arg0, arg1) {
            const ret = arg0 << arg1;
            return ret;
        },
        __wbg___wbindgen_shr_b5893fce8492f5d9: function(arg0, arg1) {
            const ret = arg0 >> arg1;
            return ret;
        },
        __wbg___wbindgen_string_get_72bdf95d3ae505b1: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_sub_0c0a13df2c647acf: function(arg0, arg1) {
            const ret = arg0 - arg1;
            return ret;
        },
        __wbg___wbindgen_throw_1506f2235d1bdba0: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg___wbindgen_try_into_number_f93cb7e60a38fe39: function(arg0) {
            let result;
            try { result = +arg0 } catch (e) { result = e }
            const ret = result;
            return ret;
        },
        __wbg___wbindgen_typeof_2f13bbeafd7a1404: function(arg0) {
            const ret = typeof arg0;
            return ret;
        },
        __wbg___wbindgen_unsigned_shr_b452b76d71863409: function(arg0, arg1) {
            const ret = arg0 >>> arg1;
            return ret;
        },
        __wbg__wbg_cb_unref_61db23ac97f16c31: function(arg0) {
            arg0._wbg_cb_unref();
        },
        __wbg_abs_3aec3895b0d04af2: function(arg0) {
            const ret = Math.abs(arg0);
            return ret;
        },
        __wbg_accept_d78715806317df4d: function(arg0, arg1) {
            const ret = arg1.accept;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_accessKeyLabel_4fe449ab93481a38: function(arg0, arg1) {
            const ret = arg1.accessKeyLabel;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_accessKey_2157f174c2811927: function(arg0, arg1) {
            const ret = arg1.accessKey;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_acos_6ca2efe8fb03aeb6: function(arg0) {
            const ret = Math.acos(arg0);
            return ret;
        },
        __wbg_acosh_16d5883f06928004: function(arg0) {
            const ret = Math.acosh(arg0);
            return ret;
        },
        __wbg_activeElement_63a85ac417bbb8d3: function(arg0) {
            const ret = arg0.activeElement;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_activeVRDisplays_ce5cdce1a1866ba2: function(arg0) {
            const ret = arg0.activeVRDisplays;
            return ret;
        },
        __wbg_addEventListener_1f1bdcafc617989b: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.addEventListener(getStringFromWasm0(arg1, arg2), arg3, arg4 !== 0, arg5 === 0xFFFFFF ? undefined : arg5 !== 0);
        }, arguments); },
        __wbg_addEventListener_7c5a0db2b2826a06: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.addEventListener(getStringFromWasm0(arg1, arg2), arg3);
        }, arguments); },
        __wbg_addEventListener_8dc53022cafa13bf: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.addEventListener(getStringFromWasm0(arg1, arg2), arg3, arg4 !== 0);
        }, arguments); },
        __wbg_addListener_9936d519754af2e7: function() { return handleError(function (arg0, arg1) {
            arg0.addListener(arg1);
        }, arguments); },
        __wbg_adoptNode_8798e246c0c3568d: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.adoptNode(arg1);
            return ret;
        }, arguments); },
        __wbg_adoptedStyleSheets_7e5cbd7387634f49: function(arg0) {
            const ret = arg0.adoptedStyleSheets;
            return ret;
        },
        __wbg_after_052c14ab599c198c: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.after(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_after_05d175894ba24a55: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.after(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_after_09485147719e6c12: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.after(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        }, arguments); },
        __wbg_after_0c39a12cea1ebb14: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.after(arg1, arg2, arg3, arg4, arg5, arg6);
        }, arguments); },
        __wbg_after_1227dcd4eade1d8f: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.after(arg1, arg2);
        }, arguments); },
        __wbg_after_17b0290b7493eccd: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.after(arg1, arg2, arg3);
        }, arguments); },
        __wbg_after_1f8bcffa5236c70a: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.after(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_after_306b81d18717d5f8: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.after(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_after_4a9b44e3725aae90: function() { return handleError(function (arg0, arg1) {
            arg0.after(arg1);
        }, arguments); },
        __wbg_after_4d2deef9eb30d7a7: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.after(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        }, arguments); },
        __wbg_after_4e6a7f6422ef7b2e: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.after(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        }, arguments); },
        __wbg_after_4ea20dbaeab9f50e: function() { return handleError(function (arg0, arg1) {
            arg0.after(arg1);
        }, arguments); },
        __wbg_after_6ad7dc0572d92f21: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.after(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        }, arguments); },
        __wbg_after_709fa28fb338fa98: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.after(arg1, arg2);
        }, arguments); },
        __wbg_after_72a1cc636c92da04: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.after(arg1, arg2, arg3, arg4);
        }, arguments); },
        __wbg_after_73547d1e125280f3: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.after(arg1, arg2, arg3, arg4, arg5, arg6);
        }, arguments); },
        __wbg_after_7e91b617799525f9: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.after(arg1, arg2, arg3, arg4);
        }, arguments); },
        __wbg_after_8f5539a83aeb2211: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.after(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        }, arguments); },
        __wbg_after_8fc9c7e044e52473: function() { return handleError(function (arg0, arg1) {
            arg0.after(...arg1);
        }, arguments); },
        __wbg_after_9799739c8ba6a493: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.after(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        }, arguments); },
        __wbg_after_ac3f1397c7dcdf38: function() { return handleError(function (arg0, arg1) {
            arg0.after(...arg1);
        }, arguments); },
        __wbg_after_af089626c0201bd0: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.after(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_after_b61c68f4cbe10889: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.after(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        }, arguments); },
        __wbg_after_bdf5d5831df41f94: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.after(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_after_c81839c1c89cf76a: function() { return handleError(function (arg0, arg1) {
            arg0.after(...arg1);
        }, arguments); },
        __wbg_after_c8b09ab38320869a: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.after(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_after_c8f494a91be7f84b: function() { return handleError(function (arg0) {
            arg0.after();
        }, arguments); },
        __wbg_after_cd5aabf440d2b0bf: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.after(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        }, arguments); },
        __wbg_after_d1d70df03665b755: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.after(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        }, arguments); },
        __wbg_after_d833fbabb2339dcc: function() { return handleError(function (arg0) {
            arg0.after();
        }, arguments); },
        __wbg_after_e048cd2020318f82: function() { return handleError(function (arg0) {
            arg0.after();
        }, arguments); },
        __wbg_after_e36e1f32b61fa480: function() { return handleError(function (arg0) {
            arg0.after();
        }, arguments); },
        __wbg_after_f1de42e8b99c13ef: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.after(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_after_f4424b2c9d96397d: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.after(arg1, arg2, arg3);
        }, arguments); },
        __wbg_after_f51ef01f6ad1115e: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.after(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        }, arguments); },
        __wbg_after_f99f0f79bde878ec: function() { return handleError(function (arg0, arg1) {
            arg0.after(...arg1);
        }, arguments); },
        __wbg_alert_87d8a87ff5b85baf: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.alert(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_alert_c28b66cfa09d76ec: function() { return handleError(function (arg0) {
            arg0.alert();
        }, arguments); },
        __wbg_align_286c6396d751c784: function(arg0, arg1) {
            const ret = arg1.align;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_align_cc36b9fcb6e6086d: function(arg0, arg1) {
            const ret = arg1.align;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_allSettled_e0f922f2ae71c12a: function(arg0) {
            const ret = Promise.allSettled(arg0);
            return ret;
        },
        __wbg_all_c90913b30f129d54: function(arg0) {
            const ret = Promise.all(arg0);
            return ret;
        },
        __wbg_allowFullscreen_96a7100616135b6d: function(arg0) {
            const ret = arg0.allowFullscreen;
            return ret;
        },
        __wbg_allowPaymentRequest_7a626e101a4e19c9: function(arg0) {
            const ret = arg0.allowPaymentRequest;
            return ret;
        },
        __wbg_altKey_4efe9bf67d712839: function(arg0) {
            const ret = arg0.altKey;
            return ret;
        },
        __wbg_altKey_77d5df8df54f3c7e: function(arg0) {
            const ret = arg0.altKey;
            return ret;
        },
        __wbg_alt_5b9972b998b72864: function(arg0, arg1) {
            const ret = arg1.alt;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_any_afc0ad9c080d0a7e: function(arg0) {
            const ret = Promise.any(arg0);
            return ret;
        },
        __wbg_appCodeName_5510d9e3b4cbf701: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.appCodeName;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_appName_9919f187569235bc: function(arg0, arg1) {
            const ret = arg1.appName;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_appVersion_cf9366a63ebdd022: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.appVersion;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_appendChild_364435158a70bd03: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.appendChild(arg1);
            return ret;
        }, arguments); },
        __wbg_appendData_a39f8766dc251034: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.appendData(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_append_0515b9640d329370: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.append(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        }, arguments); },
        __wbg_append_0639614302388588: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        }, arguments); },
        __wbg_append_0fd15671be0e2a78: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.append(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_append_122264e277162aa3: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        }, arguments); },
        __wbg_append_132dacab72810ea2: function() { return handleError(function (arg0) {
            arg0.append();
        }, arguments); },
        __wbg_append_148dec21bc6f3e08: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        }, arguments); },
        __wbg_append_17e98a938df11596: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.append(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_append_182dc6ce14b1ffef: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_append_19e0fddfc1683444: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.append(arg1, arg2, arg3);
        }, arguments); },
        __wbg_append_26e9f410927186dc: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.append(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        }, arguments); },
        __wbg_append_2f856df3fffae3dd: function() { return handleError(function (arg0, arg1) {
            arg0.append(...arg1);
        }, arguments); },
        __wbg_append_301f3fda13306a61: function() { return handleError(function (arg0, arg1) {
            arg0.append(...arg1);
        }, arguments); },
        __wbg_append_32aca416eb094368: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        }, arguments); },
        __wbg_append_380666569991e900: function() { return handleError(function (arg0) {
            arg0.append();
        }, arguments); },
        __wbg_append_3a394c6b80770ef5: function() { return handleError(function (arg0) {
            arg0.append();
        }, arguments); },
        __wbg_append_3b9a868a682444e1: function() { return handleError(function (arg0) {
            arg0.append();
        }, arguments); },
        __wbg_append_3fe8a7fd7029935a: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        }, arguments); },
        __wbg_append_45d67ce0ed8571ed: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        }, arguments); },
        __wbg_append_5035a33af51acd47: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_append_5165fc17a88cf5c5: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.append(arg1, arg2, arg3, arg4, arg5, arg6);
        }, arguments); },
        __wbg_append_53bc7a7ee5f94bb0: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.append(arg1, arg2, arg3);
        }, arguments); },
        __wbg_append_5dc8259dfa612a59: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.append(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_append_60720e67960817c5: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.append(arg1, arg2, arg3, arg4);
        }, arguments); },
        __wbg_append_650f4e301b713934: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.append(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_append_68b33e3499acca0c: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_append_68bf9a1824b76012: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.append(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_append_6a033ba5603d4f9b: function() { return handleError(function (arg0) {
            arg0.append();
        }, arguments); },
        __wbg_append_6cc2debded6c9da1: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.append(arg1, arg2);
        }, arguments); },
        __wbg_append_6dc8d148f762b7af: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.append(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        }, arguments); },
        __wbg_append_7094b55ff715782d: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        }, arguments); },
        __wbg_append_70e5d6918c8e3645: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.append(arg1, arg2, arg3);
        }, arguments); },
        __wbg_append_79c97ed0b3d60da5: function() { return handleError(function (arg0, arg1) {
            arg0.append(...arg1);
        }, arguments); },
        __wbg_append_7c3dfd3bd3e8abcd: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_append_7e7e0a67295363e7: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        }, arguments); },
        __wbg_append_81e2028e4e5d9520: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        }, arguments); },
        __wbg_append_83a3c01513d7cd0f: function() { return handleError(function (arg0, arg1) {
            arg0.append(arg1);
        }, arguments); },
        __wbg_append_8a98d3582add0a78: function() { return handleError(function (arg0, arg1) {
            arg0.append(...arg1);
        }, arguments); },
        __wbg_append_90d0b395ab0f7726: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.append(arg1, arg2, arg3, arg4);
        }, arguments); },
        __wbg_append_97275a7fbc7d1869: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.append(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_append_9c3fcac7b27470b6: function() { return handleError(function (arg0, arg1) {
            arg0.append(...arg1);
        }, arguments); },
        __wbg_append_a13e80cf72ea30ec: function() { return handleError(function (arg0, arg1) {
            arg0.append(arg1);
        }, arguments); },
        __wbg_append_b67bdfca8a337e58: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_append_b8b542107a45812f: function() { return handleError(function (arg0, arg1) {
            arg0.append(...arg1);
        }, arguments); },
        __wbg_append_bdbbd6148bc8e22b: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        }, arguments); },
        __wbg_append_be5f00d06891866e: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.append(arg1, arg2);
        }, arguments); },
        __wbg_append_c554b96a09250143: function() { return handleError(function (arg0) {
            arg0.append();
        }, arguments); },
        __wbg_append_ca4e3ce6bb197287: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.append(arg1, arg2, arg3, arg4, arg5, arg6);
        }, arguments); },
        __wbg_append_d2600e4895a03858: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.append(arg1, arg2, arg3, arg4);
        }, arguments); },
        __wbg_append_da333ea0533334cd: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.append(arg1, arg2, arg3, arg4, arg5, arg6);
        }, arguments); },
        __wbg_append_de7d784e8d80dca4: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        }, arguments); },
        __wbg_append_f6d35583d798bc90: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.append(arg1, arg2);
        }, arguments); },
        __wbg_append_f940c3c2c42c0675: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_append_fc763dc20848c4db: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.append(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        }, arguments); },
        __wbg_append_fea1de485cd7bae9: function() { return handleError(function (arg0, arg1) {
            arg0.append(arg1);
        }, arguments); },
        __wbg_apply_292b6d94e4f92b15: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.apply(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_arrayBuffer_05927079aabe6d46: function() { return handleError(function (arg0) {
            const ret = arg0.arrayBuffer();
            return ret;
        }, arguments); },
        __wbg_arrayBuffer_a0e88fd0c0e099b2: function(arg0) {
            const ret = arg0.arrayBuffer();
            return ret;
        },
        __wbg_asIntN_ae8851fd6eb19ffc: function(arg0, arg1) {
            const ret = BigInt.asIntN(arg0, arg1);
            return ret;
        },
        __wbg_asUintN_efdc404fbece5d95: function(arg0, arg1) {
            const ret = BigInt.asUintN(arg0, arg1);
            return ret;
        },
        __wbg_asin_eab7c0655efee314: function(arg0) {
            const ret = Math.asin(arg0);
            return ret;
        },
        __wbg_asinh_22407a729bf0136b: function(arg0) {
            const ret = Math.asinh(arg0);
            return ret;
        },
        __wbg_assert_0a1d4590880320ef: function(arg0, arg1, arg2, arg3) {
            console.assert(arg0 !== 0, arg1, arg2, arg3);
        },
        __wbg_assert_1803fe7fe33cefd1: function(arg0, arg1) {
            console.assert(arg0 !== 0, arg1);
        },
        __wbg_assert_22fb0100faba8a4e: function(arg0, arg1) {
            console.assert(arg0 !== 0, ...arg1);
        },
        __wbg_assert_372777923885dba9: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.assert(arg0 !== 0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_assert_6d4cd15916e5530e: function() {
            console.assert();
        },
        __wbg_assert_7cb2f7c8e616a0ab: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            console.assert(arg0 !== 0, arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        },
        __wbg_assert_7de1d8901f6d519d: function(arg0) {
            console.assert(arg0 !== 0);
        },
        __wbg_assert_a0e0be24a5c7bdd3: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.assert(arg0 !== 0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_assert_d58fe5cf44eedc2b: function(arg0, arg1, arg2, arg3, arg4) {
            console.assert(arg0 !== 0, arg1, arg2, arg3, arg4);
        },
        __wbg_assert_f2cc3468f7736036: function(arg0, arg1, arg2) {
            console.assert(arg0 !== 0, arg1, arg2);
        },
        __wbg_assign_551ec2d000f70c24: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.assign(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_asyncIterator_d7fec65292c73b67: function() {
            const ret = Symbol.asyncIterator;
            return ret;
        },
        __wbg_at_0334b7435f432553: function(arg0, arg1) {
            const ret = arg0.at(arg1);
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_at_2eef16b944bddd3f: function(arg0, arg1, arg2) {
            const ret = arg1.at(arg2);
            getDataViewMemory0().setBigInt64(arg0 + 8 * 1, isLikeNone(ret) ? BigInt(0) : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_at_38873ba7df25faf5: function(arg0, arg1) {
            const ret = arg0.at(arg1);
            return isLikeNone(ret) ? Number.MAX_SAFE_INTEGER : (ret) >>> 0;
        },
        __wbg_at_4cdb8451cbfb1f14: function(arg0, arg1) {
            const ret = arg0.at(arg1);
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_at_50cdabcd6f2e5934: function(arg0, arg1) {
            const ret = arg0.at(arg1);
            return isLikeNone(ret) ? Number.MAX_SAFE_INTEGER : (ret) >> 0;
        },
        __wbg_at_5e5aa0b7cd990963: function(arg0, arg1) {
            const ret = arg0.at(arg1);
            return isLikeNone(ret) ? Number.MAX_SAFE_INTEGER : Math.fround(ret);
        },
        __wbg_at_6503079db62012f1: function(arg0, arg1) {
            const ret = arg0.at(arg1);
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_at_7ec13c34dfac5703: function(arg0, arg1) {
            const ret = arg0.at(arg1);
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_at_99e2beab35f911a1: function(arg0, arg1, arg2) {
            const ret = arg1.at(arg2);
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_at_ab2b8b43f91220e6: function(arg0, arg1) {
            const ret = arg0.at(arg1);
            return isLikeNone(ret) ? Number.MAX_SAFE_INTEGER : Math.fround(ret);
        },
        __wbg_at_e504ad60dda7b53a: function(arg0, arg1) {
            const ret = arg0.at(arg1);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_at_ea9e844deda0defa: function(arg0, arg1) {
            const ret = arg0.at(arg1);
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_at_faf9f46894b4b772: function(arg0, arg1, arg2) {
            const ret = arg1.at(arg2);
            getDataViewMemory0().setBigInt64(arg0 + 8 * 1, isLikeNone(ret) ? BigInt(0) : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_atan2_3535bd8702b2dcae: function(arg0, arg1) {
            const ret = Math.atan2(arg0, arg1);
            return ret;
        },
        __wbg_atan_b28c188870e7a3ee: function(arg0) {
            const ret = Math.atan(arg0);
            return ret;
        },
        __wbg_atanh_e10aaac5b1a13860: function(arg0) {
            const ret = Math.atanh(arg0);
            return ret;
        },
        __wbg_atob_74340aaa01117e8f: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg1.atob(getStringFromWasm0(arg2, arg3));
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_autocomplete_8b2ca09c85245d0e: function(arg0, arg1) {
            const ret = arg1.autocomplete;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_autocomplete_ac90b6842c2c9365: function(arg0, arg1) {
            const ret = arg1.autocomplete;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_autofocus_1f8d380fc04bc895: function(arg0) {
            const ret = arg0.autofocus;
            return ret;
        },
        __wbg_autofocus_24fb35a4e15040ee: function(arg0) {
            const ret = arg0.autofocus;
            return ret;
        },
        __wbg_autofocus_b30ca22c044c6d05: function(arg0) {
            const ret = arg0.autofocus;
            return ret;
        },
        __wbg_back_3e53b5980d9fb5bd: function() { return handleError(function (arg0) {
            arg0.back();
        }, arguments); },
        __wbg_baseURI_c066bd8ea993da38: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.baseURI;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_base_name_36d86c6601e6df2c: function(arg0) {
            const ret = arg0.baseName;
            return ret;
        },
        __wbg_before_068c82c1b0aef17f: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.before(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        }, arguments); },
        __wbg_before_0cd4624d6727172d: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.before(arg1, arg2, arg3, arg4, arg5, arg6);
        }, arguments); },
        __wbg_before_0fae98fb7dc16aac: function() { return handleError(function (arg0) {
            arg0.before();
        }, arguments); },
        __wbg_before_1a77ac387b02cd4b: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.before(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_before_1d4e8a669f412b5f: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.before(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        }, arguments); },
        __wbg_before_2413585d6a9606fc: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.before(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        }, arguments); },
        __wbg_before_258ab4b53b82ad05: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.before(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_before_28b7f42708af2f6a: function() { return handleError(function (arg0) {
            arg0.before();
        }, arguments); },
        __wbg_before_2d0d537c49284d94: function() { return handleError(function (arg0, arg1) {
            arg0.before(...arg1);
        }, arguments); },
        __wbg_before_3ac473f6ee11d232: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.before(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        }, arguments); },
        __wbg_before_3d24efdef9b205d7: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.before(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        }, arguments); },
        __wbg_before_594ae09dc22d3129: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.before(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        }, arguments); },
        __wbg_before_5f8a2ba2a07b343b: function() { return handleError(function (arg0) {
            arg0.before();
        }, arguments); },
        __wbg_before_6d9d1810eaf0d752: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.before(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_before_6fdecd79a24c91a1: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.before(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        }, arguments); },
        __wbg_before_76b03046e70b951a: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.before(arg1, arg2, arg3);
        }, arguments); },
        __wbg_before_7cbc642fecabfac3: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.before(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_before_8c0cc416833a3ef6: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.before(arg1, arg2, arg3, arg4);
        }, arguments); },
        __wbg_before_8e05f50517a09bfa: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.before(arg1, arg2);
        }, arguments); },
        __wbg_before_943cc18e6987874f: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.before(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        }, arguments); },
        __wbg_before_94bd13111fd2716e: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.before(arg1, arg2, arg3);
        }, arguments); },
        __wbg_before_9ca3585dde346e16: function() { return handleError(function (arg0, arg1) {
            arg0.before(arg1);
        }, arguments); },
        __wbg_before_a807070b76ef0ded: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.before(arg1, arg2);
        }, arguments); },
        __wbg_before_aa464e573cb10b06: function() { return handleError(function (arg0, arg1) {
            arg0.before(...arg1);
        }, arguments); },
        __wbg_before_ba6cd87137526741: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.before(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        }, arguments); },
        __wbg_before_d7999940dae730ab: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.before(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_before_db2b95fd3879d8d9: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.before(arg1, arg2, arg3, arg4, arg5, arg6);
        }, arguments); },
        __wbg_before_db4ab2cce260482a: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.before(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_before_df726c700035bc50: function() { return handleError(function (arg0) {
            arg0.before();
        }, arguments); },
        __wbg_before_dfc693e834fdea2f: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.before(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        }, arguments); },
        __wbg_before_e6873eebae5f8290: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.before(arg1, arg2, arg3, arg4);
        }, arguments); },
        __wbg_before_eb766eea2df79c45: function() { return handleError(function (arg0, arg1) {
            arg0.before(...arg1);
        }, arguments); },
        __wbg_before_ee5f9e5ea0d71194: function() { return handleError(function (arg0, arg1) {
            arg0.before(arg1);
        }, arguments); },
        __wbg_before_f43b5323edba3677: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.before(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_before_fcd6991b05a41096: function() { return handleError(function (arg0, arg1) {
            arg0.before(...arg1);
        }, arguments); },
        __wbg_before_fd1f0e0631e36166: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.before(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_binaryType_d7cb4f161d865904: function(arg0) {
            const ret = arg0.binaryType;
            return (__wbindgen_enum_BinaryType.indexOf(ret) + 1 || 3) - 1;
        },
        __wbg_blob_efd99dfb6c10db4f: function() { return handleError(function (arg0) {
            const ret = arg0.blob();
            return ret;
        }, arguments); },
        __wbg_blur_596fddd0cfffdbbe: function() { return handleError(function (arg0) {
            arg0.blur();
        }, arguments); },
        __wbg_blur_fa177adf4bb4d43a: function() { return handleError(function (arg0) {
            arg0.blur();
        }, arguments); },
        __wbg_bodyUsed_bf37db8e581b9e55: function(arg0) {
            const ret = arg0.bodyUsed;
            return ret;
        },
        __wbg_body_7d5b1a2ac7f2c821: function(arg0) {
            const ret = arg0.body;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_bottom_08c48399206127ee: function(arg0) {
            const ret = arg0.bottom;
            return ret;
        },
        __wbg_btoa_d8c8894bde7cc1cd: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg1.btoa(getStringFromWasm0(arg2, arg3));
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_bubbles_6507901b016ba2aa: function(arg0) {
            const ret = arg0.bubbles;
            return ret;
        },
        __wbg_buffer_1e85ee6e37340bcf: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_buffer_3889b71323a9ab39: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_buffer_4096b4f3c7874af8: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_buffer_73b2875bda406771: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_buffer_7d3005db79ecebb9: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_buffer_98e8bb03ede54f6f: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_buffer_a1f116eb4fdb1531: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_buffer_aaddc0d44d5ab45e: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_buffer_c1a721c910556af5: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_buffer_d370c8cae5692933: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_buffer_e1c02a3985cee2d6: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_buffer_e8e5994df2664e04: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_buffer_eade5bc746c35ae3: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_buffer_f6f7a06c97944f2f: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_bufferedAmount_3a15bc0d45e69673: function(arg0) {
            const ret = arg0.bufferedAmount;
            return ret;
        },
        __wbg_button_f3dc4c82e6ee9a0c: function(arg0) {
            const ret = arg0.button;
            return ret;
        },
        __wbg_buttons_8dae14f7d9ea8c8a: function(arg0) {
            const ret = arg0.buttons;
            return ret;
        },
        __wbg_byteLength_25090a05b5b560ea: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_2c6dc3b4b85d3547: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_341237275e023d4f: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_39db6304cac5d1a5: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_5d4e1aecdd1fdfd4: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_66c1f1b2a87c9903: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_6c96d112bade8ed3: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_857b0f63ef44365a: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_a11ecb8fecf41dbd: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_a191367943ef080d: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_a3c70125b0d7065e: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_c17455c1dedcd6dc: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_ce8888385b6e4a8d: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_dd012c66d0339072: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteLength_ed24a0a2c5b037c8: function(arg0) {
            const ret = arg0.byteLength;
            return ret;
        },
        __wbg_byteOffset_0887460a6ec2ed05: function(arg0) {
            const ret = arg0.byteOffset;
            return ret;
        },
        __wbg_byteOffset_16df010cc7e1c45c: function(arg0) {
            const ret = arg0.byteOffset;
            return ret;
        },
        __wbg_byteOffset_349aa9bf0a183eca: function(arg0) {
            const ret = arg0.byteOffset;
            return ret;
        },
        __wbg_byteOffset_6540a6fd148d89db: function(arg0) {
            const ret = arg0.byteOffset;
            return ret;
        },
        __wbg_byteOffset_69c98bd0c86e30e5: function(arg0) {
            const ret = arg0.byteOffset;
            return ret;
        },
        __wbg_byteOffset_729beca75fd96883: function(arg0) {
            const ret = arg0.byteOffset;
            return ret;
        },
        __wbg_byteOffset_820bb0f42a39b97d: function(arg0) {
            const ret = arg0.byteOffset;
            return ret;
        },
        __wbg_byteOffset_8600bd80d63402c5: function(arg0) {
            const ret = arg0.byteOffset;
            return ret;
        },
        __wbg_byteOffset_a7bdb862233e6350: function(arg0) {
            const ret = arg0.byteOffset;
            return ret;
        },
        __wbg_byteOffset_c7b2ef68c84a885a: function(arg0) {
            const ret = arg0.byteOffset;
            return ret;
        },
        __wbg_byteOffset_e1f130b12072e68e: function(arg0) {
            const ret = arg0.byteOffset;
            return ret;
        },
        __wbg_byteOffset_f13548845376bb15: function(arg0) {
            const ret = arg0.byteOffset;
            return ret;
        },
        __wbg_byteOffset_f23fbd50f4adf5e1: function(arg0) {
            const ret = arg0.byteOffset;
            return ret;
        },
        __wbg_bytes_7517bc69f713b5b6: function(arg0) {
            const ret = arg0.bytes();
            return ret;
        },
        __wbg_calendar_a30aab4c600c8cd8: function(arg0) {
            const ret = arg0.calendar;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_call_40e4174f169eaca7: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.call(arg1, arg2, arg3);
            return ret;
        }, arguments); },
        __wbg_call_6e37a87ff352da3d: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.call(arg1, arg2, arg3, arg4);
            return ret;
        }, arguments); },
        __wbg_call_8a89609d89f6608a: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.call(arg1);
            return ret;
        }, arguments); },
        __wbg_call_9c758de292015997: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.call(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_canShare_f14d8708e7c8002b: function(arg0) {
            const ret = arg0.canShare();
            return ret;
        },
        __wbg_cancelAnimationFrame_fd3abe3611601214: function() { return handleError(function (arg0, arg1) {
            arg0.cancelAnimationFrame(arg1);
        }, arguments); },
        __wbg_cancelBubble_760103f953594469: function(arg0) {
            const ret = arg0.cancelBubble;
            return ret;
        },
        __wbg_cancelIdleCallback_54fc6bf909e25b1f: function(arg0, arg1) {
            arg0.cancelIdleCallback(arg1 >>> 0);
        },
        __wbg_cancelable_7bb8711eedd24c59: function(arg0) {
            const ret = arg0.cancelable;
            return ret;
        },
        __wbg_captureEvents_891482bb5ee29503: function(arg0) {
            arg0.captureEvents();
        },
        __wbg_case_first_f4a2a5a17ae03364: function(arg0) {
            const ret = arg0.caseFirst;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_cause_82cd4e3acb9e10b7: function(arg0) {
            const ret = arg0.cause;
            return ret;
        },
        __wbg_cbrt_358b721192761434: function(arg0) {
            const ret = Math.cbrt(arg0);
            return ret;
        },
        __wbg_ceil_6d7962bf5ecd0ccb: function(arg0) {
            const ret = Math.ceil(arg0);
            return ret;
        },
        __wbg_charAt_eea48aa90de1d6d7: function(arg0, arg1) {
            const ret = arg0.charAt(arg1 >>> 0);
            return ret;
        },
        __wbg_charCodeAt_464da5455a6116e4: function(arg0, arg1) {
            const ret = arg0.charCodeAt(arg1 >>> 0);
            return ret;
        },
        __wbg_charCode_5ecf7916b3dfa9a2: function(arg0) {
            const ret = arg0.charCode;
            return ret;
        },
        __wbg_characterSet_2309512b256c6e53: function(arg0, arg1) {
            const ret = arg1.characterSet;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_charset_97def05e9cbd0e7b: function(arg0, arg1) {
            const ret = arg1.charset;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_charset_d13320a5c7f2d629: function(arg0, arg1) {
            const ret = arg1.charset;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_checkValidity_5c3433aff8eb1a42: function(arg0) {
            const ret = arg0.checkValidity();
            return ret;
        },
        __wbg_checkValidity_d676b599c895f7e9: function(arg0) {
            const ret = arg0.checkValidity();
            return ret;
        },
        __wbg_checked_08fc50bf12676638: function(arg0) {
            const ret = arg0.checked;
            return ret;
        },
        __wbg_childElementCount_1be030c6de1d3b6d: function(arg0) {
            const ret = arg0.childElementCount;
            return ret;
        },
        __wbg_childElementCount_891eba4d3e406fed: function(arg0) {
            const ret = arg0.childElementCount;
            return ret;
        },
        __wbg_childElementCount_e3d1f14bd1894785: function(arg0) {
            const ret = arg0.childElementCount;
            return ret;
        },
        __wbg_className_38f30903c59cb9f1: function(arg0, arg1) {
            const ret = arg1.className;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_clearInterval_081b3f01af0f364e: function(arg0) {
            arg0.clearInterval();
        },
        __wbg_clearInterval_dd698b7aa355fbe4: function(arg0, arg1) {
            arg0.clearInterval(arg1);
        },
        __wbg_clearMarks_275d6a29183979bb: function(arg0, arg1, arg2) {
            arg0.clearMarks(getStringFromWasm0(arg1, arg2));
        },
        __wbg_clearMarks_f113ed5fae1fd1f7: function(arg0) {
            arg0.clearMarks();
        },
        __wbg_clearMeasures_51be480ed1749b77: function(arg0, arg1, arg2) {
            arg0.clearMeasures(getStringFromWasm0(arg1, arg2));
        },
        __wbg_clearMeasures_bd1d374a27685a1d: function(arg0) {
            arg0.clearMeasures();
        },
        __wbg_clearResourceTimings_065aebc84470b4f0: function(arg0) {
            arg0.clearResourceTimings();
        },
        __wbg_clearTimeout_4f7dad1647aa1690: function(arg0, arg1) {
            arg0.clearTimeout(arg1);
        },
        __wbg_clearTimeout_7ce7e8f00d4ecd42: function(arg0) {
            arg0.clearTimeout();
        },
        __wbg_clear_7a5d887b9e49d4f6: function() {
            console.clear();
        },
        __wbg_click_f4ad50d5e13b83d8: function(arg0) {
            arg0.click();
        },
        __wbg_clientHeight_a7262e398342b986: function(arg0) {
            const ret = arg0.clientHeight;
            return ret;
        },
        __wbg_clientLeft_6cc0df16b4971b04: function(arg0) {
            const ret = arg0.clientLeft;
            return ret;
        },
        __wbg_clientTop_a9eb47d894b69df9: function(arg0) {
            const ret = arg0.clientTop;
            return ret;
        },
        __wbg_clientWidth_df70d49cc7ae3e15: function(arg0) {
            const ret = arg0.clientWidth;
            return ret;
        },
        __wbg_clientX_c85019015e605e82: function(arg0) {
            const ret = arg0.clientX;
            return ret;
        },
        __wbg_clientY_e89b1cdbdb6c1772: function(arg0) {
            const ret = arg0.clientY;
            return ret;
        },
        __wbg_cloneNode_3290f1754feea3f3: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.cloneNode(arg1 !== 0);
            return ret;
        }, arguments); },
        __wbg_cloneNode_49e008a1a100ed8c: function() { return handleError(function (arg0) {
            const ret = arg0.cloneNode();
            return ret;
        }, arguments); },
        __wbg_clone_3e045e6f50c425ac: function() { return handleError(function (arg0) {
            const ret = arg0.clone();
            return ret;
        }, arguments); },
        __wbg_close_27f2a1cabf5b26c1: function() { return handleError(function (arg0) {
            arg0.close();
        }, arguments); },
        __wbg_close_76e5bacb62c204a0: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.close(arg1, getStringFromWasm0(arg2, arg3));
        }, arguments); },
        __wbg_close_9acc00cbca310439: function() { return handleError(function (arg0) {
            arg0.close();
        }, arguments); },
        __wbg_close_b3889a2dd025cb33: function() { return handleError(function (arg0, arg1) {
            arg0.close(arg1);
        }, arguments); },
        __wbg_closed_878c261e3056503c: function() { return handleError(function (arg0) {
            const ret = arg0.closed;
            return ret;
        }, arguments); },
        __wbg_closest_d8bee0814e70d0eb: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.closest(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_clz32_a946cfeda7c8ada6: function(arg0) {
            const ret = Math.clz32(arg0);
            return ret;
        },
        __wbg_codePointAt_5132731be5eb602a: function(arg0, arg1) {
            const ret = arg0.codePointAt(arg1 >>> 0);
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_codePointAt_9d2dd39377e467d2: function(arg0, arg1) {
            const ret = arg0.codePointAt(arg1 >>> 0);
            return ret;
        },
        __wbg_code_27a1f220ebdc7c36: function(arg0) {
            const ret = arg0.code;
            return ret;
        },
        __wbg_code_fda4a2b9044681ac: function(arg0, arg1) {
            const ret = arg1.code;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_collation_22806f0aae6d129d: function(arg0) {
            const ret = arg0.collation;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_colno_a19d71a989602521: function(arg0) {
            const ret = arg0.colno;
            return ret;
        },
        __wbg_cols_20252f121f853232: function(arg0) {
            const ret = arg0.cols;
            return ret;
        },
        __wbg_compareDocumentPosition_eae4dc8303fa05e2: function(arg0, arg1) {
            const ret = arg0.compareDocumentPosition(arg1);
            return ret;
        },
        __wbg_compare_d0fcfb5e63540e4b: function(arg0) {
            const ret = arg0.compare;
            return ret;
        },
        __wbg_compatMode_d93d996cfee8dfa5: function(arg0, arg1) {
            const ret = arg1.compatMode;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_compileStreaming_3b119a9a0ce4c9d3: function(arg0) {
            const ret = WebAssembly.compileStreaming(arg0);
            return ret;
        },
        __wbg_compile_fb13a573c6d2e643: function(arg0) {
            const ret = WebAssembly.compile(arg0);
            return ret;
        },
        __wbg_composedPath_e849604652cb9751: function(arg0) {
            const ret = arg0.composedPath();
            return ret;
        },
        __wbg_composed_6ea8a4af6a1c981a: function(arg0) {
            const ret = arg0.composed;
            return ret;
        },
        __wbg_concat_05b230ee7352fb49: function(arg0, arg1) {
            const ret = arg0.concat(arg1);
            return ret;
        },
        __wbg_concat_many_6c1cc8fb5911f25b: function(arg0, arg1, arg2) {
            const ret = arg0.concat_many(getArrayJsValueViewFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_confirm_912ae2a05770b2ce: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.confirm(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_confirm_be255885605b1110: function() { return handleError(function (arg0) {
            const ret = arg0.confirm();
            return ret;
        }, arguments); },
        __wbg_construct_62fbc323a33b078c: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.construct(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_construct_e0f002bb615dbba1: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.construct(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_containing_fc2132ee9a9b355f: function(arg0, arg1) {
            const ret = arg0.containing(arg1 >>> 0);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_contains_b066d97d833bedb3: function(arg0, arg1) {
            const ret = arg0.contains(arg1);
            return ret;
        },
        __wbg_contentDocument_74196edafb20230a: function(arg0) {
            const ret = arg0.contentDocument;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_contentEditable_8e16c59b4e1d806a: function(arg0, arg1) {
            const ret = arg1.contentEditable;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_contentType_b779f3e408e2fd9b: function(arg0, arg1) {
            const ret = arg1.contentType;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_contentWindow_a1cd6b9b05ee09f9: function(arg0) {
            const ret = arg0.contentWindow;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_coords_0e185e95e610a2c2: function(arg0, arg1) {
            const ret = arg1.coords;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_copyWithin_4b3325d0f367f431: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.copyWithin(arg1, arg2, arg3);
            return ret;
        },
        __wbg_copyWithin_4d5454f2296477e4: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.copyWithin(arg1, arg2, arg3);
            return ret;
        },
        __wbg_copyWithin_b471f1ca5121739f: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.copyWithin(arg1, arg2, arg3);
            return ret;
        },
        __wbg_copyWithin_bc21764cef6cbec0: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.copyWithin(arg1, arg2, arg3);
            return ret;
        },
        __wbg_copyWithin_c2cde42605bc9a69: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.copyWithin(arg1, arg2, arg3);
            return ret;
        },
        __wbg_copyWithin_c6f29a7c7d32bb40: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.copyWithin(arg1, arg2, arg3);
            return ret;
        },
        __wbg_copyWithin_e15cccb44016b84c: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.copyWithin(arg1, arg2, arg3);
            return ret;
        },
        __wbg_copyWithin_e4126e901ee1c6d5: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.copyWithin(arg1, arg2, arg3);
            return ret;
        },
        __wbg_copyWithin_e520c347d892006b: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.copyWithin(arg1, arg2, arg3);
            return ret;
        },
        __wbg_copyWithin_e8823482f08a94d7: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.copyWithin(arg1, arg2, arg3);
            return ret;
        },
        __wbg_copyWithin_ed60236ad3b8b728: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.copyWithin(arg1, arg2, arg3);
            return ret;
        },
        __wbg_copyWithin_ef0c29c8f856f824: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.copyWithin(arg1, arg2, arg3);
            return ret;
        },
        __wbg_cos_67cc9b0e32fb7098: function(arg0) {
            const ret = Math.cos(arg0);
            return ret;
        },
        __wbg_cosh_d225b9043e6b19f2: function(arg0) {
            const ret = Math.cosh(arg0);
            return ret;
        },
        __wbg_countReset_00beaf7d06a2abf3: function(arg0, arg1) {
            console.countReset(getStringFromWasm0(arg0, arg1));
        },
        __wbg_countReset_67dc67f62a4d4d00: function() {
            console.countReset();
        },
        __wbg_count_0d947284cfd59f4f: function() {
            console.count();
        },
        __wbg_count_3fc20673cf342fff: function(arg0, arg1) {
            console.count(getStringFromWasm0(arg0, arg1));
        },
        __wbg_createDocumentFragment_e10a030d56eb6d1a: function(arg0) {
            const ret = arg0.createDocumentFragment();
            return ret;
        },
        __wbg_createElementNS_149e83a2e5277d31: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            const ret = arg0.createElementNS(arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
            return ret;
        }, arguments); },
        __wbg_createElementNS_2c964c61db671f5d: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.createElementNS(arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
            return ret;
        }, arguments); },
        __wbg_createElement_8a446e3755aa90c5: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.createElement(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
            return ret;
        }, arguments); },
        __wbg_createElement_c3c16a9aa7f5cc74: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.createElement(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_createEvent_50ebd711c9ff0572: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.createEvent(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_createImageBitmap_0dddea90ca0ca9cd: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            const ret = arg0.createImageBitmap(arg1, arg2, arg3, arg4, arg5);
            return ret;
        }, arguments); },
        __wbg_createImageBitmap_3c46135a469884cc: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.createImageBitmap(arg1);
            return ret;
        }, arguments); },
        __wbg_createImageBitmap_8010fe8967b690d4: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            const ret = arg0.createImageBitmap(arg1, arg2, arg3, arg4, arg5);
            return ret;
        }, arguments); },
        __wbg_createImageBitmap_a856dd73a61bc8bb: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.createImageBitmap(arg1);
            return ret;
        }, arguments); },
        __wbg_createNSResolver_c3dbfad5d2c66407: function(arg0, arg1) {
            const ret = arg0.createNSResolver(arg1);
            return ret;
        },
        __wbg_createObjectURL_395ba916655726cd: function() { return handleError(function (arg0, arg1) {
            const ret = URL.createObjectURL(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_createTextNode_f78a04409331196b: function(arg0, arg1, arg2) {
            const ret = arg0.createTextNode(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_cssRules_e26519a35793cd05: function() { return handleError(function (arg0) {
            const ret = arg0.cssRules;
            return ret;
        }, arguments); },
        __wbg_cssText_16b200f797435a02: function(arg0, arg1) {
            const ret = arg1.cssText;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_cssText_52c77bce33a6bd56: function(arg0, arg1) {
            const ret = arg1.cssText;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_ctrlKey_1cae6780759a470d: function(arg0) {
            const ret = arg0.ctrlKey;
            return ret;
        },
        __wbg_ctrlKey_a1ca4695e4fe525a: function(arg0) {
            const ret = arg0.ctrlKey;
            return ret;
        },
        __wbg_currentScript_7cc2014a7e4a19e4: function(arg0) {
            const ret = arg0.currentScript;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_currentTarget_1bfb9f9a501418c1: function(arg0) {
            const ret = arg0.currentTarget;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_customSections_dde60527e0107ed3: function(arg0, arg1, arg2) {
            const ret = WebAssembly.Module.customSections(arg0, getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_data_b305acf81229cf6a: function(arg0, arg1) {
            const ret = arg1.data;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_data_bd354b70c783c66e: function(arg0) {
            const ret = arg0.data;
            return ret;
        },
        __wbg_days_53f00b2ce7cab42f: function(arg0, arg1) {
            const ret = arg1.days;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_debug_35ce98ebe1e2423a: function(arg0, arg1, arg2) {
            console.debug(arg0, arg1, arg2);
        },
        __wbg_debug_363dfe29b68c2340: function(arg0) {
            console.debug(...arg0);
        },
        __wbg_debug_6d96d354ecb8cdb3: function(arg0, arg1, arg2, arg3) {
            console.debug(arg0, arg1, arg2, arg3);
        },
        __wbg_debug_78b457f1effb3792: function(arg0) {
            console.debug(arg0);
        },
        __wbg_debug_7b64a0132849c2f1: function(arg0, arg1) {
            console.debug(arg0, arg1);
        },
        __wbg_debug_8d3f9de93537142c: function(arg0, arg1, arg2, arg3, arg4) {
            console.debug(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_debug_94ddac3efabc53c1: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.debug(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_debug_99582486d72bc93e: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.debug(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_debug_dae231c80d92da97: function() {
            console.debug();
        },
        __wbg_decodeURIComponent_b3ca91f9662a251a: function() { return handleError(function (arg0, arg1) {
            const ret = decodeURIComponent(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_decodeURI_10ff378b94dfddfc: function() { return handleError(function (arg0, arg1) {
            const ret = decodeURI(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_defaultChecked_f11dd465a7dc99c7: function(arg0) {
            const ret = arg0.defaultChecked;
            return ret;
        },
        __wbg_defaultPrevented_13123f6a48ff9d10: function(arg0) {
            const ret = arg0.defaultPrevented;
            return ret;
        },
        __wbg_defaultValue_d8a30f87e64fda0f: function(arg0, arg1) {
            const ret = arg1.defaultValue;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_defaultValue_e6dd6d4a19ad0154: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.defaultValue;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_defaultView_d062822747467313: function(arg0) {
            const ret = arg0.defaultView;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_deleteData_c04662057c158466: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.deleteData(arg1 >>> 0, arg2 >>> 0);
        }, arguments); },
        __wbg_deleteRule_1a3d098f618c64f7: function() { return handleError(function (arg0, arg1) {
            arg0.deleteRule(arg1 >>> 0);
        }, arguments); },
        __wbg_detached_155c582041fdd9f0: function(arg0) {
            const ret = arg0.detached;
            return ret;
        },
        __wbg_detail_0c5e4fe5bceef66e: function(arg0) {
            const ret = arg0.detail;
            return ret;
        },
        __wbg_devicePixelRatio_dab1a0b7ea57b26a: function(arg0) {
            const ret = arg0.devicePixelRatio;
            return ret;
        },
        __wbg_dir_17f0b6323a5460c5: function() {
            console.dir();
        },
        __wbg_dir_46c2121bacf3a453: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.dir(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_dir_488d1d321cba41fe: function(arg0, arg1) {
            const ret = arg1.dir;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_dir_4c65df873aeb66e7: function(arg0, arg1) {
            const ret = arg1.dir;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_dir_4e800beefa2d9754: function(arg0, arg1) {
            console.dir(arg0, arg1);
        },
        __wbg_dir_820ae79e25547a3c: function(arg0, arg1, arg2) {
            console.dir(arg0, arg1, arg2);
        },
        __wbg_dir_89cf7781e26f9761: function(arg0, arg1, arg2, arg3) {
            console.dir(arg0, arg1, arg2, arg3);
        },
        __wbg_dir_90e32cf06975fbb5: function(arg0) {
            console.dir(arg0);
        },
        __wbg_dir_a6e167bc7cd11a71: function(arg0, arg1, arg2, arg3, arg4) {
            console.dir(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_dir_afd97bee6a46bac8: function(arg0) {
            console.dir(...arg0);
        },
        __wbg_dir_b42b4955517bda12: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.dir(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_direction_b9849befc990dd20: function(arg0) {
            const ret = arg0.direction;
            return ret;
        },
        __wbg_dirxml_0c16e58eb79f0440: function(arg0, arg1, arg2) {
            console.dirxml(arg0, arg1, arg2);
        },
        __wbg_dirxml_2b1de4776a31279c: function() {
            console.dirxml();
        },
        __wbg_dirxml_56d4ffa76a9f43a2: function(arg0, arg1, arg2, arg3) {
            console.dirxml(arg0, arg1, arg2, arg3);
        },
        __wbg_dirxml_65e35f312c56d788: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.dirxml(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_dirxml_7add75ea20600ec3: function(arg0, arg1) {
            console.dirxml(arg0, arg1);
        },
        __wbg_dirxml_80e84e673e84b77e: function(arg0) {
            console.dirxml(arg0);
        },
        __wbg_dirxml_d374e4fdb62d097c: function(arg0, arg1, arg2, arg3, arg4) {
            console.dirxml(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_dirxml_e32d894d70239179: function(arg0) {
            console.dirxml(...arg0);
        },
        __wbg_dirxml_f2e4b88eb2962fff: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.dirxml(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_disabled_1ba557c599ff8239: function(arg0) {
            const ret = arg0.disabled;
            return ret;
        },
        __wbg_disabled_95c4fd5bee0ab949: function(arg0) {
            const ret = arg0.disabled;
            return ret;
        },
        __wbg_disabled_a8ac7f9ef379a376: function(arg0) {
            const ret = arg0.disabled;
            return ret;
        },
        __wbg_disabled_c5c0ca44b7eab049: function(arg0) {
            const ret = arg0.disabled;
            return ret;
        },
        __wbg_disconnect_baa8c650bef8508f: function(arg0) {
            arg0.disconnect();
        },
        __wbg_dispatchEvent_9e344ca144dd9545: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.dispatchEvent(arg1);
            return ret;
        }, arguments); },
        __wbg_doNotTrack_ba8a90bc9e49f780: function(arg0, arg1) {
            const ret = arg1.doNotTrack;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_documentElement_19b770919f3ea991: function(arg0) {
            const ret = arg0.documentElement;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_documentURI_4623d7f4fa4a1706: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.documentURI;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_document_aceb08cd6489baf5: function(arg0) {
            const ret = arg0.document;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_download_d18c0a667d1f5173: function(arg0, arg1) {
            const ret = arg1.download;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_draggable_75a7e48e978a71df: function(arg0) {
            const ret = arg0.draggable;
            return ret;
        },
        __wbg_elementFromPoint_8fc68a904c6f137a: function(arg0, arg1, arg2) {
            const ret = arg0.elementFromPoint(arg1, arg2);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_elementsFromPoint_4295ee2824bfedb4: function(arg0, arg1, arg2) {
            const ret = arg0.elementsFromPoint(arg1, arg2);
            return ret;
        },
        __wbg_enableStyleSheetsForSet_fc227586692655b3: function(arg0, arg1, arg2) {
            arg0.enableStyleSheetsForSet(arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2));
        },
        __wbg_encodeURIComponent_9ff907ad9d03c7bb: function(arg0, arg1) {
            const ret = encodeURIComponent(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_encodeURI_73e549e385ee85c1: function(arg0, arg1) {
            const ret = encodeURI(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_endsWith_336e94ddd48e4ea6: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.endsWith(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        },
        __wbg_entries_04b37a02507f1713: function(arg0) {
            const ret = Object.entries(arg0);
            return ret;
        },
        __wbg_error_2435c1aa0a89d9f0: function(arg0, arg1, arg2) {
            console.error(arg0, arg1, arg2);
        },
        __wbg_error_4b325a99ebd1039e: function(arg0) {
            console.error(...arg0);
        },
        __wbg_error_618ca9e20c753e50: function() {
            const ret = Response.error();
            return ret;
        },
        __wbg_error_7826ac63c3abdda9: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.error(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_error_78ff5b3a29b770e0: function(arg0) {
            console.error(arg0);
        },
        __wbg_error_8684686d7121a8be: function(arg0, arg1, arg2, arg3, arg4) {
            console.error(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_error_945eecb226e7d377: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.error(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_error_9ad1450feb5d541d: function(arg0, arg1, arg2, arg3) {
            console.error(arg0, arg1, arg2, arg3);
        },
        __wbg_error_caf3e7df11b6ba4a: function(arg0) {
            const ret = arg0.error;
            return ret;
        },
        __wbg_error_d01fe6ffb3676b31: function() {
            console.error();
        },
        __wbg_error_f48cb636668f83b3: function(arg0, arg1) {
            console.error(arg0, arg1);
        },
        __wbg_errors_98fe4900870bd2e7: function(arg0) {
            const ret = arg0.errors;
            return ret;
        },
        __wbg_escape_cdcab10e29e660c5: function(arg0, arg1) {
            const ret = escape(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_eval_35600f795897d127: function() { return handleError(function (arg0, arg1) {
            const ret = eval(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_eventPhase_5dcdf099b87d996a: function(arg0) {
            const ret = arg0.eventPhase;
            return ret;
        },
        __wbg_event_4614273722115ee6: function(arg0) {
            const ret = arg0.event;
            return ret;
        },
        __wbg_exception_1694bde899f7dc09: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.exception(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_exception_2c34b16eb3c94761: function(arg0, arg1, arg2, arg3, arg4) {
            console.exception(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_exception_34aa6a1ed1cd6871: function(arg0) {
            console.exception(...arg0);
        },
        __wbg_exception_406e37e47276fec9: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.exception(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_exception_66162f8da5a2cb9b: function(arg0, arg1, arg2, arg3) {
            console.exception(arg0, arg1, arg2, arg3);
        },
        __wbg_exception_6eac9a1494ed4313: function(arg0, arg1, arg2) {
            console.exception(arg0, arg1, arg2);
        },
        __wbg_exception_8ab7ec2bc52ec184: function(arg0, arg1) {
            console.exception(arg0, arg1);
        },
        __wbg_exception_bbe9107c5a4d5605: function() {
            console.exception();
        },
        __wbg_exception_e0d7f25cfad47f4a: function(arg0) {
            console.exception(arg0);
        },
        __wbg_exec_930f6ed50a31c7a4: function(arg0, arg1, arg2) {
            const ret = arg0.exec(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_exitFullscreen_dca21b75fcbef72e: function(arg0) {
            arg0.exitFullscreen();
        },
        __wbg_exitPointerLock_8f9416ce7933f14f: function(arg0) {
            arg0.exitPointerLock();
        },
        __wbg_exp_a8d3fe1b16ee4e89: function(arg0) {
            const ret = Math.exp(arg0);
            return ret;
        },
        __wbg_expm1_daac81a341c97704: function(arg0) {
            const ret = Math.expm1(arg0);
            return ret;
        },
        __wbg_exports_08ad6ad6f5f4c5e5: function(arg0) {
            const ret = WebAssembly.Module.exports(arg0);
            return ret;
        },
        __wbg_exports_0adaaea039d27694: function(arg0) {
            const ret = arg0.exports;
            return ret;
        },
        __wbg_extensions_2cfc0f25571aee99: function(arg0, arg1) {
            const ret = arg1.extensions;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_fetch_8420348c88fa7037: function(arg0, arg1, arg2) {
            const ret = arg0.fetch(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_filename_da7f231da77ad0bf: function(arg0, arg1) {
            const ret = arg1.filename;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_fill_12f774ee82b9d584: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.fill(arg1, arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_fill_16161e62be6b8ccf: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.fill(arg1, arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_fill_212bebd9b189087c: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.fill(arg1, arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_fill_23709a79d4ec27a3: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.fill(arg1, arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_fill_45dd6ad7b6dd28db: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.fill(arg1, arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_fill_49e64437997b4970: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.fill(arg1, arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_fill_590a836e67a1c3c0: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.fill(BigInt.asUintN(64, arg1), arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_fill_79a7b310e4fd4a82: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.fill(arg1, arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_fill_7c95ac318ac0c9ec: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.fill(arg1, arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_fill_b355ade30f28e5e6: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.fill(arg1, arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_fill_d26c1acf1c4a349a: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.fill(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_fill_d2beaa11e63fadbd: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.fill(arg1, arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_firstChild_b074ab013a0373a1: function(arg0) {
            const ret = arg0.firstChild;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_firstElementChild_58209b1ce3b098f9: function(arg0) {
            const ret = arg0.firstElementChild;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_firstElementChild_5db49bb5a1d71cc1: function(arg0) {
            const ret = arg0.firstElementChild;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_firstElementChild_ee712de68c51df4f: function(arg0) {
            const ret = arg0.firstElementChild;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_first_day_75ea0832fdb5cff7: function(arg0) {
            const ret = arg0.firstDay;
            return ret;
        },
        __wbg_flags_97a61678e073c856: function(arg0) {
            const ret = arg0.flags;
            return ret;
        },
        __wbg_floor_599db317f6c5627f: function(arg0) {
            const ret = Math.floor(arg0);
            return ret;
        },
        __wbg_focus_45b2f9483661ea93: function() { return handleError(function (arg0) {
            arg0.focus();
        }, arguments); },
        __wbg_focus_a3a6c806c5bccea1: function() { return handleError(function (arg0) {
            arg0.focus();
        }, arguments); },
        __wbg_for_efbf4b1041ab2b98: function(arg0, arg1) {
            const ret = Symbol.for(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_formAction_e605661f58d6d388: function(arg0, arg1) {
            const ret = arg1.formAction;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_formData_35602decaee8ef17: function() { return handleError(function (arg0) {
            const ret = arg0.formData();
            return ret;
        }, arguments); },
        __wbg_formEnctype_b1f27e7bb28b0825: function(arg0, arg1) {
            const ret = arg1.formEnctype;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_formMethod_802eb26c1ca608da: function(arg0, arg1) {
            const ret = arg1.formMethod;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_formNoValidate_d428b704dc45de02: function(arg0) {
            const ret = arg0.formNoValidate;
            return ret;
        },
        __wbg_formTarget_f215ec673f29180f: function(arg0, arg1) {
            const ret = arg1.formTarget;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_formatRangeToParts_6aadd0adfa5163bd: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.formatRangeToParts(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_formatRangeToParts_87cf09feac146fa1: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.formatRangeToParts(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_formatRange_0d94414c5982701e: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.formatRange(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_formatRange_9aa33d993384aae8: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.formatRange(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_formatToParts_102acd8436964e6c: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.formatToParts(arg1, getStringFromWasm0(arg2, arg3));
            return ret;
        },
        __wbg_formatToParts_148d890bdf27be65: function(arg0, arg1) {
            const ret = arg0.formatToParts(arg1);
            return ret;
        },
        __wbg_formatToParts_38e041c5cba1bf26: function(arg0, arg1) {
            const ret = arg0.formatToParts(arg1);
            return ret;
        },
        __wbg_formatToParts_a35cc1da4a10438d: function(arg0, arg1) {
            const ret = arg0.formatToParts(arg1);
            return ret;
        },
        __wbg_formatToParts_e389f7701e251374: function(arg0, arg1) {
            const ret = arg0.formatToParts(arg1);
            return ret;
        },
        __wbg_format_2f1abd6a749a51d3: function(arg0, arg1) {
            const ret = arg0.format(arg1);
            return ret;
        },
        __wbg_format_32d2d88df14e2649: function(arg0, arg1) {
            const ret = arg0.format(arg1);
            return ret;
        },
        __wbg_format_37483f332bc6e9bd: function(arg0) {
            const ret = arg0.format;
            return ret;
        },
        __wbg_format_9ea1734ff3af7b9c: function(arg0) {
            const ret = arg0.format;
            return ret;
        },
        __wbg_format_e2ec5cc6c39c6784: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.format(arg1, getStringFromWasm0(arg2, arg3));
            return ret;
        },
        __wbg_forward_eeed37582baadfaf: function() { return handleError(function (arg0) {
            arg0.forward();
        }, arguments); },
        __wbg_frameBorder_a76fe31b830369dd: function(arg0, arg1) {
            const ret = arg1.frameBorder;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_frameElement_bb8dd5e835e5f1e8: function() { return handleError(function (arg0) {
            const ret = arg0.frameElement;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_frames_91aa0d4098796088: function() { return handleError(function (arg0) {
            const ret = arg0.frames;
            return ret;
        }, arguments); },
        __wbg_fromCharCode_081845a2fa07bebc: function(arg0, arg1) {
            const ret = String.fromCharCode(arg0 >>> 0, arg1 >>> 0);
            return ret;
        },
        __wbg_fromCharCode_10ef5d1e3b3543e4: function(arg0, arg1, arg2, arg3) {
            const ret = String.fromCharCode(arg0 >>> 0, arg1 >>> 0, arg2 >>> 0, arg3 >>> 0);
            return ret;
        },
        __wbg_fromCharCode_257f1e65e2a67840: function(arg0) {
            const ret = String.fromCharCode(arg0 >>> 0);
            return ret;
        },
        __wbg_fromCharCode_6094c96851611f11: function(arg0, arg1) {
            const ret = String.fromCharCode(...getArrayU16FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_fromCharCode_6ddb811c59ca7a05: function(arg0, arg1, arg2, arg3, arg4) {
            const ret = String.fromCharCode(arg0 >>> 0, arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4 >>> 0);
            return ret;
        },
        __wbg_fromCharCode_86b67e67d48fb813: function(arg0, arg1, arg2) {
            const ret = String.fromCharCode(arg0 >>> 0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_fromCodePoint_0f414c2a2279ef11: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = String.fromCodePoint(arg0 >>> 0, arg1 >>> 0, arg2 >>> 0, arg3 >>> 0);
            return ret;
        }, arguments); },
        __wbg_fromCodePoint_57dd9d3bc1678a2f: function() { return handleError(function (arg0, arg1) {
            const ret = String.fromCodePoint(arg0 >>> 0, arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_fromCodePoint_5c5781da87ec0bc3: function() { return handleError(function (arg0) {
            const ret = String.fromCodePoint(arg0 >>> 0);
            return ret;
        }, arguments); },
        __wbg_fromCodePoint_c67cec12d7910e78: function() { return handleError(function (arg0, arg1) {
            const ret = String.fromCodePoint(...getArrayU32FromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_fromCodePoint_edc32f771eb3076a: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = String.fromCodePoint(arg0 >>> 0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        }, arguments); },
        __wbg_fromCodePoint_f29a1c28c3302ccb: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = String.fromCodePoint(arg0 >>> 0, arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4 >>> 0);
            return ret;
        }, arguments); },
        __wbg_fromEntries_a30edd4fb741901f: function() { return handleError(function (arg0) {
            const ret = Object.fromEntries(arg0);
            return ret;
        }, arguments); },
        __wbg_from_d300fe49deab18f5: function(arg0) {
            const ret = Array.from(arg0);
            return ret;
        },
        __wbg_fround_fdbdd7b2247a2888: function(arg0) {
            const ret = Math.fround(arg0);
            return ret;
        },
        __wbg_fullscreenElement_82487ff822dce583: function(arg0) {
            const ret = arg0.fullscreenElement;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_fullscreenEnabled_0f71b7aa93bda844: function(arg0) {
            const ret = arg0.fullscreenEnabled;
            return ret;
        },
        __wbg_fullscreen_8b7e4b6a3f44db9d: function(arg0) {
            const ret = arg0.fullscreen;
            return ret;
        },
        __wbg_getAnimations_d994e4d5e613b3ca: function(arg0) {
            const ret = arg0.getAnimations();
            return ret;
        },
        __wbg_getArg_ba2ae21eac9f1219: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.getArg(arg1, arg2 >>> 0);
            return ret;
        }, arguments); },
        __wbg_getAttributeNS_b69d3de106f27155: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            const ret = arg1.getAttributeNS(arg2 === 0 ? undefined : getStringFromWasm0(arg2, arg3), getStringFromWasm0(arg4, arg5));
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_getAttributeNames_70bbf4b798021d24: function(arg0) {
            const ret = arg0.getAttributeNames();
            return ret;
        },
        __wbg_getAttribute_27f7a0d74339fd41: function(arg0, arg1, arg2, arg3) {
            const ret = arg1.getAttribute(getStringFromWasm0(arg2, arg3));
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_getBoundingClientRect_93c2750834277567: function(arg0) {
            const ret = arg0.getBoundingClientRect();
            return ret;
        },
        __wbg_getBoxQuads_92693111733b80d3: function() { return handleError(function (arg0) {
            const ret = arg0.getBoxQuads();
            return ret;
        }, arguments); },
        __wbg_getBoxQuads_db46f29aa66e31ef: function() { return handleError(function (arg0) {
            const ret = arg0.getBoxQuads();
            return ret;
        }, arguments); },
        __wbg_getBoxQuads_f6a7c7e074b13369: function() { return handleError(function (arg0) {
            const ret = arg0.getBoxQuads();
            return ret;
        }, arguments); },
        __wbg_getCalendars_5c6d4a07dea6ea95: function(arg0) {
            const ret = arg0.getCalendars();
            return ret;
        },
        __wbg_getCanonicalLocales_66ecbff35cbe22ca: function(arg0) {
            const ret = Intl.getCanonicalLocales(arg0);
            return ret;
        },
        __wbg_getCoalescedEvents_99adec3626cff3a6: function(arg0) {
            const ret = arg0.getCoalescedEvents();
            return ret;
        },
        __wbg_getCollations_99584f6c1749f6ae: function(arg0) {
            const ret = arg0.getCollations();
            return ret;
        },
        __wbg_getComputedStyle_025ec99c5c7baad4: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.getComputedStyle(arg1, getStringFromWasm0(arg2, arg3));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_getComputedStyle_c59f58a15bc6a800: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.getComputedStyle(arg1);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_getContext_469d34698d869fc1: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.getContext(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_getContext_5b39fff76491fded: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.getContext(getStringFromWasm0(arg1, arg2), arg3);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_getDate_3d2f964145b3449d: function(arg0) {
            const ret = arg0.getDate();
            return ret;
        },
        __wbg_getDay_36952f7a4e4bf5c2: function(arg0) {
            const ret = arg0.getDay();
            return ret;
        },
        __wbg_getElementById_b5e59e463108ae17: function(arg0, arg1, arg2) {
            const ret = arg0.getElementById(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_getElementById_c35b4b7d270d161d: function(arg0, arg1, arg2) {
            const ret = arg0.getElementById(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_getEntriesByName_4166c037ccd9621b: function(arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.getEntriesByName(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
            return ret;
        },
        __wbg_getEntriesByName_b413e31e8596c455: function(arg0, arg1, arg2) {
            const ret = arg0.getEntriesByName(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_getEntriesByType_bc993b6f4dd5590a: function(arg0, arg1, arg2) {
            const ret = arg0.getEntriesByType(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_getEntries_72bbc4de9d717275: function(arg0) {
            const ret = arg0.getEntries();
            return ret;
        },
        __wbg_getFloat16_3d7c1ded1241a698: function(arg0, arg1) {
            const ret = arg0.getFloat16(arg1 >>> 0);
            return ret;
        },
        __wbg_getFloat16_bba31fc150a60ea4: function(arg0, arg1, arg2) {
            const ret = arg0.getFloat16(arg1 >>> 0, arg2 !== 0);
            return ret;
        },
        __wbg_getFloat32_05898646a7e289a2: function(arg0, arg1, arg2) {
            const ret = arg0.getFloat32(arg1 >>> 0, arg2 !== 0);
            return ret;
        },
        __wbg_getFloat32_e2196f5b93a8c55f: function(arg0, arg1) {
            const ret = arg0.getFloat32(arg1 >>> 0);
            return ret;
        },
        __wbg_getFloat64_73254aa9dd0e8132: function(arg0, arg1) {
            const ret = arg0.getFloat64(arg1 >>> 0);
            return ret;
        },
        __wbg_getFloat64_7bda219e7cfaea62: function(arg0, arg1, arg2) {
            const ret = arg0.getFloat64(arg1 >>> 0, arg2 !== 0);
            return ret;
        },
        __wbg_getFullYear_8b1ba9a648a4de7f: function(arg0) {
            const ret = arg0.getFullYear();
            return ret;
        },
        __wbg_getGamepads_10e740c08fdc24fc: function() { return handleError(function (arg0) {
            const ret = arg0.getGamepads();
            return ret;
        }, arguments); },
        __wbg_getHourCycles_619860909e57007b: function(arg0) {
            const ret = arg0.getHourCycles();
            return ret;
        },
        __wbg_getHours_91ac680ae491b8ea: function(arg0) {
            const ret = arg0.getHours();
            return ret;
        },
        __wbg_getInt16_f1b24cb5628d3835: function(arg0, arg1, arg2) {
            const ret = arg0.getInt16(arg1 >>> 0, arg2 !== 0);
            return ret;
        },
        __wbg_getInt16_f43a16bef2640523: function(arg0, arg1) {
            const ret = arg0.getInt16(arg1 >>> 0);
            return ret;
        },
        __wbg_getInt32_68380922a20b1126: function(arg0, arg1) {
            const ret = arg0.getInt32(arg1 >>> 0);
            return ret;
        },
        __wbg_getInt32_92b3fd2b3b217b5f: function(arg0, arg1, arg2) {
            const ret = arg0.getInt32(arg1 >>> 0, arg2 !== 0);
            return ret;
        },
        __wbg_getInt8_ced7a8b733baa9c4: function(arg0, arg1) {
            const ret = arg0.getInt8(arg1 >>> 0);
            return ret;
        },
        __wbg_getMilliseconds_5fa104a44b95037e: function(arg0) {
            const ret = arg0.getMilliseconds();
            return ret;
        },
        __wbg_getMinutes_c1c2573becc0c7b5: function(arg0) {
            const ret = arg0.getMinutes();
            return ret;
        },
        __wbg_getModifierState_63ccfbb841a8bf5a: function(arg0, arg1, arg2) {
            const ret = arg0.getModifierState(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_getModifierState_ced5f34a7bc361f3: function(arg0, arg1, arg2) {
            const ret = arg0.getModifierState(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_getMonth_44bdf67c99f2ed79: function(arg0) {
            const ret = arg0.getMonth();
            return ret;
        },
        __wbg_getNumberingSystems_c3f754eb0ea19fb3: function(arg0) {
            const ret = arg0.getNumberingSystems();
            return ret;
        },
        __wbg_getPropertyPriority_f12534f7f91b7346: function(arg0, arg1, arg2, arg3) {
            const ret = arg1.getPropertyPriority(getStringFromWasm0(arg2, arg3));
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_getPropertyValue_dbbb77f232017e4d: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg1.getPropertyValue(getStringFromWasm0(arg2, arg3));
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_getPrototypeOf_abee07b8ce2b932f: function() { return handleError(function (arg0) {
            const ret = Reflect.getPrototypeOf(arg0);
            return ret;
        }, arguments); },
        __wbg_getPrototypeOf_af285f52dda56f77: function(arg0) {
            const ret = Object.getPrototypeOf(arg0);
            return ret;
        },
        __wbg_getRootNode_8883da02d1e52749: function(arg0) {
            const ret = arg0.getRootNode();
            return ret;
        },
        __wbg_getSVGDocument_61c209806166745c: function(arg0) {
            const ret = arg0.getSVGDocument();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_getSeconds_2716239745eecd0a: function(arg0) {
            const ret = arg0.getSeconds();
            return ret;
        },
        __wbg_getTextInfo_2cf15a8f93e4f25a: function() { return handleError(function (arg0) {
            const ret = arg0.getTextInfo();
            return ret;
        }, arguments); },
        __wbg_getTimeZones_20f783624663cd16: function(arg0) {
            const ret = arg0.getTimeZones();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_getTime_00b3f7db575e4ef5: function(arg0) {
            const ret = arg0.getTime();
            return ret;
        },
        __wbg_getTimezoneOffset_08e2892156231088: function(arg0) {
            const ret = arg0.getTimezoneOffset();
            return ret;
        },
        __wbg_getUTCDate_d14ee4f1d3eb06e0: function(arg0) {
            const ret = arg0.getUTCDate();
            return ret;
        },
        __wbg_getUTCDay_a518b69ba60752bf: function(arg0) {
            const ret = arg0.getUTCDay();
            return ret;
        },
        __wbg_getUTCFullYear_a2d4f171284031fb: function(arg0) {
            const ret = arg0.getUTCFullYear();
            return ret;
        },
        __wbg_getUTCHours_bae4f5e0e2a50a54: function(arg0) {
            const ret = arg0.getUTCHours();
            return ret;
        },
        __wbg_getUTCMilliseconds_569878819662690e: function(arg0) {
            const ret = arg0.getUTCMilliseconds();
            return ret;
        },
        __wbg_getUTCMinutes_bb0b12bb4e4f1486: function(arg0) {
            const ret = arg0.getUTCMinutes();
            return ret;
        },
        __wbg_getUTCMonth_9c08bcf471ff0d1a: function(arg0) {
            const ret = arg0.getUTCMonth();
            return ret;
        },
        __wbg_getUTCSeconds_71549fa6e6599f29: function(arg0) {
            const ret = arg0.getUTCSeconds();
            return ret;
        },
        __wbg_getUint16_1a65f8177aaa3ff0: function(arg0, arg1) {
            const ret = arg0.getUint16(arg1 >>> 0);
            return ret;
        },
        __wbg_getUint16_c3de8831d52531a1: function(arg0, arg1, arg2) {
            const ret = arg0.getUint16(arg1 >>> 0, arg2 !== 0);
            return ret;
        },
        __wbg_getUint32_630cc22fba3094a8: function(arg0, arg1) {
            const ret = arg0.getUint32(arg1 >>> 0);
            return ret;
        },
        __wbg_getUint32_a97257996264f19e: function(arg0, arg1, arg2) {
            const ret = arg0.getUint32(arg1 >>> 0, arg2 !== 0);
            return ret;
        },
        __wbg_getUint8_88667d6c91a7273d: function(arg0, arg1) {
            const ret = arg0.getUint8(arg1 >>> 0);
            return ret;
        },
        __wbg_getVRDisplays_ed05a4912871f143: function() { return handleError(function (arg0) {
            const ret = arg0.getVRDisplays();
            return ret;
        }, arguments); },
        __wbg_getWeekInfo_3643f4875be83d5e: function() { return handleError(function (arg0) {
            const ret = arg0.getWeekInfo();
            return ret;
        }, arguments); },
        __wbg_get_1f8f054ddbaa7db2: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_2b48c7d0d006a781: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_32c9a2dfc942c7a4: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_get_4b0a27a178a3d16b: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_get_4b424f7a583f8651: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.get(arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_get_4f7f45dd79de4c1f: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.get(arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_get_7568896ecd66403b: function(arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        },
        __wbg_get_793c2a868ad3ae60: function(arg0, arg1, arg2) {
            const ret = arg1[arg2 >>> 0];
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_get_8baf556ec6b77a8a: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_get_95fa8cf1fb21bb82: function(arg0, arg1, arg2) {
            const ret = arg0[getStringFromWasm0(arg1, arg2)];
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_get_afbe3deebc0254ed: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_calendar_84db200eb6ee998f: function(arg0) {
            const ret = arg0.calendar;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_get_case_first_c53767acc4064cf3: function(arg0) {
            const ret = arg0.caseFirst;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_CollatorCaseFirst.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_cause_589760892b633813: function(arg0) {
            const ret = arg0.cause;
            return ret;
        },
        __wbg_get_collation_8c168102aa2add75: function(arg0) {
            const ret = arg0.collation;
            return ret;
        },
        __wbg_get_compact_display_78ac325d64b3ae12: function(arg0) {
            const ret = arg0.compactDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_CompactDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_currency_1ff580c454d100fa: function(arg0) {
            const ret = arg0.currency;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_get_currency_display_84702745b5a8b6e4: function(arg0) {
            const ret = arg0.currencyDisplay;
            return isLikeNone(ret) ? 5 : ((__wbindgen_enum_CurrencyDisplay.indexOf(ret) + 1 || 5) - 1);
        },
        __wbg_get_currency_sign_9d87abdbef4b88f8: function(arg0) {
            const ret = arg0.currencySign;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_CurrencySign.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_date_style_8d1b3a8e8516555b: function(arg0) {
            const ret = arg0.dateStyle;
            return isLikeNone(ret) ? 5 : ((__wbindgen_enum_DateTimeStyle.indexOf(ret) + 1 || 5) - 1);
        },
        __wbg_get_day_7f0af6e4bb493406: function(arg0) {
            const ret = arg0.day;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_DayFormat.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_day_period_f955da7293430943: function(arg0) {
            const ret = arg0.dayPeriod;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_DayPeriodFormat.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_days_c72bc15f6a35d5ae: function(arg0) {
            const ret = arg0.days;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_DurationUnitStyle.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_days_display_ac90ebd84187b0fd: function(arg0) {
            const ret = arg0.daysDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_DurationUnitDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_de6a0f7d4d18a304: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_era_20d367c20972d1db: function(arg0) {
            const ret = arg0.era;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_EraFormat.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_fallback_fe77341818611829: function(arg0) {
            const ret = arg0.fallback;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_DisplayNamesFallback.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_fractional_digits_12c8dac28784778f: function(arg0) {
            const ret = arg0.fractionalDigits;
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_get_fractional_second_digits_57aeafe3cf503e14: function(arg0) {
            const ret = arg0.fractionalSecondDigits;
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_get_granularity_28ead81b1fc17e2c: function(arg0) {
            const ret = arg0.granularity;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_SegmenterGranularity.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_hour12_5f7126d088b33f68: function(arg0) {
            const ret = arg0.hour12;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg_get_hour_6a11567c035a6368: function(arg0) {
            const ret = arg0.hour;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_NumericFormat.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_hour_cycle_8b8f236ca7f076ff: function(arg0) {
            const ret = arg0.hourCycle;
            return isLikeNone(ret) ? 5 : ((__wbindgen_enum_HourCycle.indexOf(ret) + 1 || 5) - 1);
        },
        __wbg_get_hours_d7d5506e8efa446d: function(arg0) {
            const ret = arg0.hours;
            return isLikeNone(ret) ? 6 : ((__wbindgen_enum_DurationTimeUnitStyle.indexOf(ret) + 1 || 6) - 1);
        },
        __wbg_get_hours_display_20d8a639731fe9f5: function(arg0) {
            const ret = arg0.hoursDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_DurationUnitDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_ignore_punctuation_ac46a2cda906583c: function(arg0) {
            const ret = arg0.ignorePunctuation;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg_get_index_19693d65e963f8b2: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_index_2d7d568da8770e46: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_index_35f33911e596c876: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_index_7994de0a1069e644: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_index_83d7d62a85da02ac: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_index_ab123d64c89e3156: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_index_ae03aafc4236a066: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_index_as_f32_0308589b467b79f8: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_index_c7d0ea1874d52489: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_index_e0c6fbfb5fccde23: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_index_f1ac1290e08992ca: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_index_f898c2fd35c723c2: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_language_display_1f6ce1e4fb711251: function(arg0) {
            const ret = arg0.languageDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_DisplayNamesLanguageDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_locale_0b01155227d5f503: function(arg0) {
            const ret = arg0.locale;
            return ret;
        },
        __wbg_get_locale_0d4e16e390219d21: function(arg0) {
            const ret = arg0.locale;
            return ret;
        },
        __wbg_get_locale_4a303906af4561f7: function(arg0) {
            const ret = arg0.locale;
            return ret;
        },
        __wbg_get_locale_5e0defca77020dad: function(arg0) {
            const ret = arg0.locale;
            return ret;
        },
        __wbg_get_locale_611b74efd3dccf61: function(arg0) {
            const ret = arg0.locale;
            return ret;
        },
        __wbg_get_locale_72f369c0a186c00b: function(arg0) {
            const ret = arg0.locale;
            return ret;
        },
        __wbg_get_locale_787121bb9fbb78eb: function(arg0) {
            const ret = arg0.locale;
            return ret;
        },
        __wbg_get_locale_f21a0a4d098f7a73: function(arg0) {
            const ret = arg0.locale;
            return ret;
        },
        __wbg_get_locale_f6f5b20c8f43ccd1: function(arg0) {
            const ret = arg0.locale;
            return ret;
        },
        __wbg_get_locale_matcher_1a5c3c57255c83ff: function(arg0) {
            const ret = arg0.localeMatcher;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_LocaleMatcher.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_locale_matcher_2201dbaf10219028: function(arg0) {
            const ret = arg0.localeMatcher;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_LocaleMatcher.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_locale_matcher_29d845ca375f7fdf: function(arg0) {
            const ret = arg0.localeMatcher;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_LocaleMatcher.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_locale_matcher_4e85bd9323b2e9d4: function(arg0) {
            const ret = arg0.localeMatcher;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_LocaleMatcher.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_locale_matcher_92285553fe8b5041: function(arg0) {
            const ret = arg0.localeMatcher;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_LocaleMatcher.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_locale_matcher_945b1ba125466cc2: function(arg0) {
            const ret = arg0.localeMatcher;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_LocaleMatcher.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_locale_matcher_9685a99f5c39a357: function(arg0) {
            const ret = arg0.localeMatcher;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_LocaleMatcher.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_locale_matcher_d84e6d4a3d8a6882: function(arg0) {
            const ret = arg0.localeMatcher;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_LocaleMatcher.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_locale_matcher_e775e7b2dccbb2ce: function(arg0) {
            const ret = arg0.localeMatcher;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_LocaleMatcher.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_locale_matcher_f697b58332d99fbe: function(arg0) {
            const ret = arg0.localeMatcher;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_LocaleMatcher.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_maximum_fraction_digits_7543d547349ece3f: function(arg0) {
            const ret = arg0.maximumFractionDigits;
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_get_maximum_fraction_digits_e9c60a6d33427b74: function(arg0) {
            const ret = arg0.maximumFractionDigits;
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_get_maximum_significant_digits_155c5aa969360156: function(arg0) {
            const ret = arg0.maximumSignificantDigits;
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_get_maximum_significant_digits_ff76401eaa33760e: function(arg0) {
            const ret = arg0.maximumSignificantDigits;
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_get_microseconds_display_73894c8cdcf547b4: function(arg0) {
            const ret = arg0.microsecondsDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_DurationUnitDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_microseconds_f23087f4055dbe16: function(arg0) {
            const ret = arg0.microseconds;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_DurationUnitStyle.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_milliseconds_2302b51d3708e80d: function(arg0) {
            const ret = arg0.milliseconds;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_DurationUnitStyle.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_milliseconds_display_6ecfe0ae6e809a93: function(arg0) {
            const ret = arg0.millisecondsDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_DurationUnitDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_minimum_fraction_digits_71ef663b3e97ef13: function(arg0) {
            const ret = arg0.minimumFractionDigits;
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_get_minimum_fraction_digits_ba19c2d767e40689: function(arg0) {
            const ret = arg0.minimumFractionDigits;
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_get_minimum_integer_digits_7f6ba6dadb4724cb: function(arg0) {
            const ret = arg0.minimumIntegerDigits;
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_get_minimum_integer_digits_8c9bac9891b84d5a: function(arg0) {
            const ret = arg0.minimumIntegerDigits;
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_get_minimum_significant_digits_6c6b77b7d0225cc8: function(arg0) {
            const ret = arg0.minimumSignificantDigits;
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_get_minimum_significant_digits_d012e9a9e3cf10fc: function(arg0) {
            const ret = arg0.minimumSignificantDigits;
            return isLikeNone(ret) ? 0xFFFFFF : ret;
        },
        __wbg_get_minute_47a22958518f2d7e: function(arg0) {
            const ret = arg0.minute;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_NumericFormat.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_minutes_15d2b904a9b00a35: function(arg0) {
            const ret = arg0.minutes;
            return isLikeNone(ret) ? 6 : ((__wbindgen_enum_DurationTimeUnitStyle.indexOf(ret) + 1 || 6) - 1);
        },
        __wbg_get_minutes_display_49ae3b469014494b: function(arg0) {
            const ret = arg0.minutesDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_DurationUnitDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_month_341437ec40f0bec2: function(arg0) {
            const ret = arg0.month;
            return isLikeNone(ret) ? 6 : ((__wbindgen_enum_MonthFormat.indexOf(ret) + 1 || 6) - 1);
        },
        __wbg_get_months_90b046b486f5194e: function(arg0) {
            const ret = arg0.months;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_DurationUnitStyle.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_months_display_2639efa2e249921c: function(arg0) {
            const ret = arg0.monthsDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_DurationUnitDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_nanoseconds_8eb8fe80b81414f4: function(arg0) {
            const ret = arg0.nanoseconds;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_DurationUnitStyle.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_nanoseconds_display_662845a9dba6fb19: function(arg0) {
            const ret = arg0.nanosecondsDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_DurationUnitDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_notation_6df3ed7c98864165: function(arg0) {
            const ret = arg0.notation;
            return isLikeNone(ret) ? 5 : ((__wbindgen_enum_NumberFormatNotation.indexOf(ret) + 1 || 5) - 1);
        },
        __wbg_get_numbering_system_6f95818cec214457: function(arg0) {
            const ret = arg0.numberingSystem;
            return ret;
        },
        __wbg_get_numbering_system_82bfb77ba92fe8a3: function(arg0) {
            const ret = arg0.numberingSystem;
            return ret;
        },
        __wbg_get_numbering_system_b0ec5301ebe71a85: function(arg0) {
            const ret = arg0.numberingSystem;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_get_numbering_system_b5f8e862658289b8: function(arg0) {
            const ret = arg0.numberingSystem;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_get_numeric_25eacc580c028434: function(arg0) {
            const ret = arg0.numeric;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg_get_numeric_755a94bace2bd34f: function(arg0) {
            const ret = arg0.numeric;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_RelativeTimeFormatNumeric.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_plural_categories_c0192dd73d42e19e: function(arg0) {
            const ret = arg0.pluralCategories;
            return ret;
        },
        __wbg_get_rounding_increment_7c07edff86b06889: function(arg0) {
            const ret = arg0.roundingIncrement;
            return isLikeNone(ret) ? Number.MAX_SAFE_INTEGER : (ret) >>> 0;
        },
        __wbg_get_rounding_increment_9c492825ef71d6a6: function(arg0) {
            const ret = arg0.roundingIncrement;
            return isLikeNone(ret) ? Number.MAX_SAFE_INTEGER : (ret) >>> 0;
        },
        __wbg_get_rounding_mode_30a9d61f27dc286c: function(arg0) {
            const ret = arg0.roundingMode;
            return isLikeNone(ret) ? 10 : ((__wbindgen_enum_RoundingMode.indexOf(ret) + 1 || 10) - 1);
        },
        __wbg_get_rounding_mode_f02a6355c46ef715: function(arg0) {
            const ret = arg0.roundingMode;
            return isLikeNone(ret) ? 10 : ((__wbindgen_enum_RoundingMode.indexOf(ret) + 1 || 10) - 1);
        },
        __wbg_get_rounding_priority_0239728c85eaaf3e: function(arg0) {
            const ret = arg0.roundingPriority;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_RoundingPriority.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_rounding_priority_1899c2ccab84b2dc: function(arg0) {
            const ret = arg0.roundingPriority;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_RoundingPriority.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_second_0b0c06c956d1050d: function(arg0) {
            const ret = arg0.second;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_NumericFormat.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_seconds_2c13411090383c14: function(arg0) {
            const ret = arg0.seconds;
            return isLikeNone(ret) ? 6 : ((__wbindgen_enum_DurationTimeUnitStyle.indexOf(ret) + 1 || 6) - 1);
        },
        __wbg_get_seconds_display_c1735f2dd8c400c1: function(arg0) {
            const ret = arg0.secondsDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_DurationUnitDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_sensitivity_4fdbce3cfb514763: function(arg0) {
            const ret = arg0.sensitivity;
            return isLikeNone(ret) ? 5 : ((__wbindgen_enum_CollatorSensitivity.indexOf(ret) + 1 || 5) - 1);
        },
        __wbg_get_sign_display_5fa8a4cae51a573a: function(arg0) {
            const ret = arg0.signDisplay;
            return isLikeNone(ret) ? 5 : ((__wbindgen_enum_SignDisplay.indexOf(ret) + 1 || 5) - 1);
        },
        __wbg_get_style_0077bda5971a46b1: function(arg0) {
            const ret = arg0.style;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_DisplayNamesStyle.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_style_30b972f7ff86d4f5: function(arg0) {
            const ret = arg0.style;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_RelativeTimeFormatStyle.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_style_91f14f8383c79439: function(arg0) {
            const ret = arg0.style;
            return isLikeNone(ret) ? 5 : ((__wbindgen_enum_NumberFormatStyle.indexOf(ret) + 1 || 5) - 1);
        },
        __wbg_get_style_ba55ae1ed5e87e01: function(arg0) {
            const ret = arg0.style;
            return isLikeNone(ret) ? 5 : ((__wbindgen_enum_DurationFormatStyle.indexOf(ret) + 1 || 5) - 1);
        },
        __wbg_get_style_bca9899f99ac7cfb: function(arg0) {
            const ret = arg0.style;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_ListFormatStyle.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_time_style_b4d5580ff68fc43a: function(arg0) {
            const ret = arg0.timeStyle;
            return isLikeNone(ret) ? 5 : ((__wbindgen_enum_DateTimeStyle.indexOf(ret) + 1 || 5) - 1);
        },
        __wbg_get_time_zone_b80cd2afbac70690: function(arg0) {
            const ret = arg0.timeZone;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_get_time_zone_name_7280bf5b202c7e59: function(arg0) {
            const ret = arg0.timeZoneName;
            return isLikeNone(ret) ? 7 : ((__wbindgen_enum_TimeZoneNameFormat.indexOf(ret) + 1 || 7) - 1);
        },
        __wbg_get_trailing_zero_display_842794579703917e: function(arg0) {
            const ret = arg0.trailingZeroDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_TrailingZeroDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_trailing_zero_display_991e0a06c147c712: function(arg0) {
            const ret = arg0.trailingZeroDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_TrailingZeroDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_type_599a3dca5999dbe3: function(arg0) {
            const ret = arg0.type;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_PluralRulesType.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_type_df785c30e462ad5b: function(arg0, arg1) {
            const ret = arg1.type;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_get_type_eec37a850788ada7: function(arg0) {
            const ret = arg0.type;
            return isLikeNone(ret) ? 7 : ((__wbindgen_enum_DisplayNamesType.indexOf(ret) + 1 || 7) - 1);
        },
        __wbg_get_type_f75545ed4e25002c: function(arg0) {
            const ret = arg0.type;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_ListFormatType.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_unit_329027e82839ef40: function(arg0) {
            const ret = arg0.unit;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_get_unit_display_fe8d32a1e00edc4e: function(arg0) {
            const ret = arg0.unitDisplay;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_UnitDisplay.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_usage_c8b97c8ec25ca5a4: function(arg0) {
            const ret = arg0.usage;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_CollatorUsage.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_use_grouping_e9a49478733d4ac6: function(arg0) {
            const ret = arg0.useGrouping;
            return isLikeNone(ret) ? 6 : ((__wbindgen_enum_UseGrouping.indexOf(ret) + 1 || 6) - 1);
        },
        __wbg_get_weekday_ce0164cac73f8dfe: function(arg0) {
            const ret = arg0.weekday;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_WeekdayFormat.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_weeks_display_00882b6e42727648: function(arg0) {
            const ret = arg0.weeksDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_DurationUnitDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_weeks_e22dfb9658cc0df3: function(arg0) {
            const ret = arg0.weeks;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_DurationUnitStyle.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_year_1e59e1edbacbf7c7: function(arg0) {
            const ret = arg0.year;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_YearFormat.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_get_years_736fbadcbdba8790: function(arg0) {
            const ret = arg0.years;
            return isLikeNone(ret) ? 4 : ((__wbindgen_enum_DurationUnitStyle.indexOf(ret) + 1 || 4) - 1);
        },
        __wbg_get_years_display_b1304db102fbda9b: function(arg0) {
            const ret = arg0.yearsDisplay;
            return isLikeNone(ret) ? 3 : ((__wbindgen_enum_DurationUnitDisplay.indexOf(ret) + 1 || 3) - 1);
        },
        __wbg_global_76fc7205e8fc8dcc: function(arg0) {
            const ret = arg0.global;
            return ret;
        },
        __wbg_go_03b0313c93439e49: function() { return handleError(function (arg0) {
            arg0.go();
        }, arguments); },
        __wbg_go_dac51820659b4ff0: function() { return handleError(function (arg0, arg1) {
            arg0.go(arg1);
        }, arguments); },
        __wbg_groupCollapsed_0abc5a83a9821a09: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.groupCollapsed(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_groupCollapsed_17c5aa67ed415712: function(arg0, arg1) {
            console.groupCollapsed(arg0, arg1);
        },
        __wbg_groupCollapsed_4bb16096d27160ed: function(arg0, arg1, arg2) {
            console.groupCollapsed(arg0, arg1, arg2);
        },
        __wbg_groupCollapsed_5ddc016501854aea: function(arg0, arg1, arg2, arg3) {
            console.groupCollapsed(arg0, arg1, arg2, arg3);
        },
        __wbg_groupCollapsed_677993a2f0f68ce2: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.groupCollapsed(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_groupCollapsed_899a74feff183b4c: function(arg0) {
            console.groupCollapsed(...arg0);
        },
        __wbg_groupCollapsed_b5ad4706ec75ff38: function(arg0) {
            console.groupCollapsed(arg0);
        },
        __wbg_groupCollapsed_ce49e11a09b32ef8: function(arg0, arg1, arg2, arg3, arg4) {
            console.groupCollapsed(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_groupCollapsed_fb6b60d81a2fb00a: function() {
            console.groupCollapsed();
        },
        __wbg_groupEnd_7357ef5f9f85e264: function() {
            console.groupEnd();
        },
        __wbg_group_0b709e0976747a47: function(arg0, arg1, arg2, arg3, arg4) {
            console.group(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_group_0c41cd1184070c2a: function(arg0, arg1, arg2, arg3) {
            console.group(arg0, arg1, arg2, arg3);
        },
        __wbg_group_1bfadff036174cbd: function(arg0) {
            console.group(...arg0);
        },
        __wbg_group_8c9e5ebe1ad5eeda: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.group(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_group_9a5d2c331ab7b356: function() {
            console.group();
        },
        __wbg_group_bbf011da78db4fc0: function(arg0, arg1, arg2) {
            console.group(arg0, arg1, arg2);
        },
        __wbg_group_c43dab473a7ba727: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.group(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_group_d57c03ea8a96b9ce: function(arg0, arg1) {
            console.group(arg0, arg1);
        },
        __wbg_group_e6ee5c6d2a26a2e4: function(arg0) {
            console.group(arg0);
        },
        __wbg_groups_d69869d0278a35f2: function(arg0) {
            const ret = arg0.groups;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_grow_4f5ad007fb3c0ef5: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.grow(arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_grow_61c1bbadc6f77d72: function() { return handleError(function (arg0, arg1) {
            arg0.grow(arg1 >>> 0);
        }, arguments); },
        __wbg_grow_675cdab766668c89: function(arg0, arg1) {
            const ret = arg0.grow(arg1 >>> 0);
            return ret;
        },
        __wbg_grow_6f372e0436ec3c4c: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.grow(arg1 >>> 0, arg2);
            return ret;
        }, arguments); },
        __wbg_growable_080e09d4d3bb6ef4: function(arg0) {
            const ret = arg0.growable;
            return ret;
        },
        __wbg_hardwareConcurrency_6ea7d2267444bcf4: function(arg0) {
            const ret = arg0.hardwareConcurrency;
            return ret;
        },
        __wbg_hasAttributeNS_a1cfff81e5d0a5d3: function(arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.hasAttributeNS(arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
            return ret;
        },
        __wbg_hasAttribute_8a0e80dea64a024f: function(arg0, arg1, arg2) {
            const ret = arg0.hasAttribute(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_hasAttributes_fbf923532d182d37: function(arg0) {
            const ret = arg0.hasAttributes();
            return ret;
        },
        __wbg_hasChildNodes_78ec3cba596b8fa4: function(arg0) {
            const ret = arg0.hasChildNodes();
            return ret;
        },
        __wbg_hasFocus_b62a61fddcdf30e9: function() { return handleError(function (arg0) {
            const ret = arg0.hasFocus();
            return ret;
        }, arguments); },
        __wbg_hasInstance_4707f618c630dfa6: function() {
            const ret = Symbol.hasInstance;
            return ret;
        },
        __wbg_hasPointerCapture_228c88b254dc9273: function(arg0, arg1) {
            const ret = arg0.hasPointerCapture(arg1);
            return ret;
        },
        __wbg_has_73740b27f436fed3: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.has(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_hash_1ee4bd6fa886f85b: function(arg0, arg1) {
            const ret = arg1.hash;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_hash_82f20440d27754b0: function(arg0, arg1) {
            const ret = arg1.hash;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_hash_943e3a1af6d171d2: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.hash;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_head_79cb26ea82b99acd: function(arg0) {
            const ret = arg0.head;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_height_6a894c1fa1807273: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_height_87fd6bb4af652876: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_height_8e3b6ac1a60655fb: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_height_8e6603d2bea90dc1: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_height_d98b32be52adfc7f: function(arg0, arg1) {
            const ret = arg1.height;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_height_ef5b5950872773b5: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_hidden_3ee33107d5a58db0: function(arg0) {
            const ret = arg0.hidden;
            return ret;
        },
        __wbg_hidden_c191ab678665d0e8: function(arg0) {
            const ret = arg0.hidden;
            return ret;
        },
        __wbg_hidePopover_d4d31fb93096cf1d: function() { return handleError(function (arg0) {
            arg0.hidePopover();
        }, arguments); },
        __wbg_history_58333fc71c2a7689: function() { return handleError(function (arg0) {
            const ret = arg0.history;
            return ret;
        }, arguments); },
        __wbg_host_701b015b5b504f94: function(arg0, arg1) {
            const ret = arg1.host;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_host_89fe6c6d29b2c68d: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.host;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_host_e515a64aedbfa09d: function(arg0, arg1) {
            const ret = arg1.host;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_hostname_311e2a4c6de4f6fc: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.hostname;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_hostname_343be66448f7e5f2: function(arg0, arg1) {
            const ret = arg1.hostname;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_hostname_6363c2680df01c7d: function(arg0, arg1) {
            const ret = arg1.hostname;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_hour_cycle_b49676a51be764c6: function(arg0) {
            const ret = arg0.hourCycle;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_hours_556213989092ffd5: function(arg0, arg1) {
            const ret = arg1.hours;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_href_127efbdb4d7466d7: function(arg0, arg1) {
            const ret = arg1.href;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_href_35aa8f65e8143b5e: function(arg0, arg1) {
            const ret = arg1.href;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_href_cfdd5eb8694d45e8: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.href;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_href_d48ea07dc40f8d57: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.href;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_hreflang_8684ec1b79dfcb6c: function(arg0, arg1) {
            const ret = arg1.hreflang;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_hypot_40e9aa010d14d388: function(arg0, arg1) {
            const ret = Math.hypot(arg0, arg1);
            return ret;
        },
        __wbg_id_3c85189284ee6544: function(arg0, arg1) {
            const ret = arg1.id;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_ignoreCase_c70e70111986b757: function(arg0) {
            const ret = arg0.ignoreCase;
            return ret;
        },
        __wbg_importNode_180de7106f07d9a1: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.importNode(arg1, arg2 !== 0);
            return ret;
        }, arguments); },
        __wbg_importNode_b1e4936353b8e87c: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.importNode(arg1);
            return ret;
        }, arguments); },
        __wbg_imports_31bae21745f7f680: function(arg0) {
            const ret = WebAssembly.Module.imports(arg0);
            return ret;
        },
        __wbg_imul_48adc8c007c20e0e: function(arg0, arg1) {
            const ret = Math.imul(arg0, arg1);
            return ret;
        },
        __wbg_includes_9de74ab73da1154e: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.includes(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        },
        __wbg_indeterminate_631981d5416e50ac: function(arg0) {
            const ret = arg0.indeterminate;
            return ret;
        },
        __wbg_indexOf_f0427735bb27a37b: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.indexOf(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        },
        __wbg_index_44bf4f5d7a5e9946: function(arg0) {
            const ret = arg0.index;
            return ret;
        },
        __wbg_index_b13350ca901b99d8: function(arg0) {
            const ret = arg0.index;
            return ret;
        },
        __wbg_inert_0bd91a1cdd1e743d: function(arg0) {
            const ret = arg0.inert;
            return ret;
        },
        __wbg_info_0ce4bcb4d78da9f6: function(arg0) {
            console.info(...arg0);
        },
        __wbg_info_47724252aef90b86: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.info(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_info_5cfb3f6c22c53cf9: function(arg0, arg1, arg2, arg3) {
            console.info(arg0, arg1, arg2, arg3);
        },
        __wbg_info_5dccc7e419a9a1eb: function() {
            console.info();
        },
        __wbg_info_7e3d3a0607d6567b: function(arg0, arg1, arg2, arg3, arg4) {
            console.info(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_info_9973f6cfc37aa58b: function(arg0, arg1, arg2) {
            console.info(arg0, arg1, arg2);
        },
        __wbg_info_9b3a3334dd956150: function(arg0, arg1) {
            console.info(arg0, arg1);
        },
        __wbg_info_af7f45292ba9b0ea: function(arg0) {
            console.info(arg0);
        },
        __wbg_info_c0d3c528f8fb9a58: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.info(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_initEvent_06503a66703d44f2: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.initEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0);
        },
        __wbg_initEvent_cb1fb476a0c61763: function(arg0, arg1, arg2) {
            arg0.initEvent(getStringFromWasm0(arg1, arg2));
        },
        __wbg_initEvent_da3770f0a14c8da3: function(arg0, arg1, arg2, arg3) {
            arg0.initEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0);
        },
        __wbg_initKeyboardEvent_1afa0941fd64895f: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.initKeyboardEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0);
        }, arguments); },
        __wbg_initKeyboardEvent_3a91e4a4bff90b8b: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.initKeyboardEvent(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_initKeyboardEvent_4ae1898f9c788065: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11) {
            arg0.initKeyboardEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, getStringFromWasm0(arg6, arg7), arg8 >>> 0, arg9 !== 0, arg10 !== 0, arg11 !== 0);
        }, arguments); },
        __wbg_initKeyboardEvent_576115245f6c7ecb: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.initKeyboardEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, getStringFromWasm0(arg6, arg7), arg8 >>> 0, arg9 !== 0);
        }, arguments); },
        __wbg_initKeyboardEvent_620fae117c05645d: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.initKeyboardEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, getStringFromWasm0(arg6, arg7));
        }, arguments); },
        __wbg_initKeyboardEvent_642abcf3173633f6: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.initKeyboardEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0);
        }, arguments); },
        __wbg_initKeyboardEvent_ada54d10437a6970: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.initKeyboardEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, getStringFromWasm0(arg6, arg7), arg8 >>> 0);
        }, arguments); },
        __wbg_initKeyboardEvent_b3882f4192d1b382: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.initKeyboardEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, getStringFromWasm0(arg6, arg7), arg8 >>> 0, arg9 !== 0, arg10 !== 0, arg11 !== 0, arg12 !== 0);
        }, arguments); },
        __wbg_initKeyboardEvent_c659caf5e2a172e3: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.initKeyboardEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, getStringFromWasm0(arg6, arg7), arg8 >>> 0, arg9 !== 0, arg10 !== 0);
        }, arguments); },
        __wbg_initKeyboardEvent_f975b7345a1ab2d3: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.initKeyboardEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5);
        }, arguments); },
        __wbg_initMessageEvent_27cb18f8dd0ab8b7: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.initMessageEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, getStringFromWasm0(arg6, arg7), getStringFromWasm0(arg8, arg9), arg10);
        },
        __wbg_initMessageEvent_2a65dfcaacf3d6c4: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11) {
            arg0.initMessageEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, getStringFromWasm0(arg6, arg7), getStringFromWasm0(arg8, arg9), arg10, arg11);
        },
        __wbg_initMessageEvent_607a9097a1962272: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.initMessageEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5);
        },
        __wbg_initMessageEvent_754dcd27c390d38f: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.initMessageEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, getStringFromWasm0(arg6, arg7));
        },
        __wbg_initMessageEvent_a678e06da2cd4f79: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.initMessageEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0);
        },
        __wbg_initMessageEvent_bad8ae6e3940ae72: function(arg0, arg1, arg2) {
            arg0.initMessageEvent(getStringFromWasm0(arg1, arg2));
        },
        __wbg_initMessageEvent_bf19ee0ad73b8636: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.initMessageEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, getStringFromWasm0(arg6, arg7), getStringFromWasm0(arg8, arg9));
        },
        __wbg_initMessageEvent_c871816cad5ab594: function(arg0, arg1, arg2, arg3) {
            arg0.initMessageEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0);
        },
        __wbg_initMouseEvent_00eb1c724f684428: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, arg6, arg7, arg8, arg9, arg10);
        },
        __wbg_initMouseEvent_0d10dd8529efb09d: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, arg6, arg7, arg8, arg9);
        },
        __wbg_initMouseEvent_0d7e1c446b243c76: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, arg6, arg7, arg8);
        },
        __wbg_initMouseEvent_34e24953927af8ab: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, arg6, arg7, arg8, arg9, arg10, arg11 !== 0, arg12 !== 0);
        },
        __wbg_initMouseEvent_5052085c54a7d95c: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0);
        },
        __wbg_initMouseEvent_8e2cc5b7377daea7: function(arg0, arg1, arg2, arg3) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0);
        },
        __wbg_initMouseEvent_99f9ae7e86c3e6b0: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5);
        },
        __wbg_initMouseEvent_a07f99d1ca13ae5a: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14, arg15, arg16) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, arg6, arg7, arg8, arg9, arg10, arg11 !== 0, arg12 !== 0, arg13 !== 0, arg14 !== 0, arg15, arg16);
        },
        __wbg_initMouseEvent_a38320c5d95a2492: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, arg6, arg7, arg8, arg9, arg10, arg11 !== 0, arg12 !== 0, arg13 !== 0, arg14 !== 0);
        },
        __wbg_initMouseEvent_b318c4b284f8e074: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, arg6, arg7, arg8, arg9, arg10, arg11 !== 0, arg12 !== 0, arg13 !== 0);
        },
        __wbg_initMouseEvent_b7ee230a1563a8d8: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, arg6, arg7);
        },
        __wbg_initMouseEvent_dcdb4f5468fc4b2b: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, arg6);
        },
        __wbg_initMouseEvent_e1b26f27a39fb070: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, arg6, arg7, arg8, arg9, arg10, arg11 !== 0);
        },
        __wbg_initMouseEvent_e764d49a22cd9749: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14, arg15) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, arg6, arg7, arg8, arg9, arg10, arg11 !== 0, arg12 !== 0, arg13 !== 0, arg14 !== 0, arg15);
        },
        __wbg_initMouseEvent_fd928d07646fb88c: function(arg0, arg1, arg2) {
            arg0.initMouseEvent(getStringFromWasm0(arg1, arg2));
        },
        __wbg_initUIEvent_03f2436bbac78402: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.initUIEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0);
        },
        __wbg_initUIEvent_5cbe2a46dd366ed7: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.initUIEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5, arg6);
        },
        __wbg_initUIEvent_a24b2eb5f0f65db3: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.initUIEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0, arg4 !== 0, arg5);
        },
        __wbg_initUIEvent_a4fb8ef580eee58f: function(arg0, arg1, arg2) {
            arg0.initUIEvent(getStringFromWasm0(arg1, arg2));
        },
        __wbg_initUIEvent_c1f31c13f05dd08f: function(arg0, arg1, arg2, arg3) {
            arg0.initUIEvent(getStringFromWasm0(arg1, arg2), arg3 !== 0);
        },
        __wbg_innerHTML_22d06520ba322d88: function(arg0, arg1) {
            const ret = arg1.innerHTML;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_innerHeight_8b6ee2571dbedb9d: function() { return handleError(function (arg0) {
            const ret = arg0.innerHeight;
            return ret;
        }, arguments); },
        __wbg_innerText_2719d4ffe8a3a29c: function(arg0, arg1) {
            const ret = arg1.innerText;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_innerWidth_7475bec19f48fe43: function() { return handleError(function (arg0) {
            const ret = arg0.innerWidth;
            return ret;
        }, arguments); },
        __wbg_inputEncoding_ad4f9cac9e6674d9: function(arg0, arg1) {
            const ret = arg1.inputEncoding;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_inputMode_dcfbad484c9c50dd: function(arg0, arg1) {
            const ret = arg1.inputMode;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_input_45d965673e2f8083: function() {
            const ret = RegExp.input;
            return ret;
        },
        __wbg_input_8c3a5e4d8159a500: function(arg0) {
            const ret = arg0.input;
            return ret;
        },
        __wbg_input_ff74acc6cdb5f6c3: function(arg0) {
            const ret = arg0.input;
            return ret;
        },
        __wbg_insertAdjacentElement_8081a065ecd3f35c: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.insertAdjacentElement(getStringFromWasm0(arg1, arg2), arg3);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_insertAdjacentHTML_bf19ef0461d48668: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.insertAdjacentHTML(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_insertAdjacentText_1e88b2c05f456ffa: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.insertAdjacentText(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_insertBefore_41123fdf1b69b43d: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.insertBefore(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_insertData_24019384364a6d93: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.insertData(arg1 >>> 0, getStringFromWasm0(arg2, arg3));
        }, arguments); },
        __wbg_insertRule_a305957623bbb124: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.insertRule(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_insertRule_df7eafadd190c6e4: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.insertRule(getStringFromWasm0(arg1, arg2), arg3 >>> 0);
            return ret;
        }, arguments); },
        __wbg_instanceof_AggregateError_3154a1470252de04: function(arg0) {
            let result;
            try {
                result = arg0 instanceof AggregateError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ArrayBufferOptions_6a0f63de0a4edecd: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ArrayBufferOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ArrayBuffer_8f49811467741499: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ArrayBuffer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_BigInt64Array_26a04d28e5570130: function(arg0) {
            let result;
            try {
                result = arg0 instanceof BigInt64Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_BigInt_ab5d3a4a80c9f279: function(arg0) {
            let result;
            try {
                result = arg0 instanceof BigInt;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_BigUint64Array_cf5fffc89681b491: function(arg0) {
            let result;
            try {
                result = arg0 instanceof BigUint64Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_BlobPropertyBag_de1d478a7279404a: function(arg0) {
            let result;
            try {
                result = arg0 instanceof BlobPropertyBag;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Blob_f6321ce92d2740fd: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Blob;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Boolean_3e490c3b07ffcbdb: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Boolean;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_CharacterData_a7cab4ea5d79d719: function(arg0) {
            let result;
            try {
                result = arg0 instanceof CharacterData;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_CloseEvent_cc3c99fd4c0a5f61: function(arg0) {
            let result;
            try {
                result = arg0 instanceof CloseEvent;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_CollatorOptions_b686ae1ab11db78b: function(arg0) {
            let result;
            try {
                result = arg0 instanceof CollatorOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Collator_b4a3321638a0afab: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Intl.Collator;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_CompileError_e8f50533f0989fe2: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WebAssembly.CompileError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_CssRuleList_023fbca2c421c231: function(arg0) {
            let result;
            try {
                result = arg0 instanceof CSSRuleList;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_CssRule_b0fdb8fbd6233a18: function(arg0) {
            let result;
            try {
                result = arg0 instanceof CSSRule;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_CssStyleDeclaration_d82cd7a4dfef3b25: function(arg0) {
            let result;
            try {
                result = arg0 instanceof CSSStyleDeclaration;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_CssStyleRule_10c3304b88e09f76: function(arg0) {
            let result;
            try {
                result = arg0 instanceof CSSStyleRule;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_CssStyleSheet_6c0c469ab75cf3a9: function(arg0) {
            let result;
            try {
                result = arg0 instanceof CSSStyleSheet;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DataView_fd63ac599aa203b7: function(arg0) {
            let result;
            try {
                result = arg0 instanceof DataView;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DateTimeFormatOptions_75f21f5ffefbb53a: function(arg0) {
            let result;
            try {
                result = arg0 instanceof DateTimeFormatOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DateTimeFormatPart_0e354b27b31aced6: function(arg0) {
            let result;
            try {
                result = arg0 instanceof DateTimeFormatPart;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DateTimeFormat_dbd7f45165dd5337: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Intl.DateTimeFormat;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DateTimeRangeFormatPart_b811059b33f492bb: function(arg0) {
            let result;
            try {
                result = arg0 instanceof DateTimeRangeFormatPart;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Date_878a2457ad173ac2: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Date;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DisplayNamesOptions_e692e9819f778d2a: function(arg0) {
            let result;
            try {
                result = arg0 instanceof DisplayNamesOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DisplayNames_a94fc4efec9d4fed: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Intl.DisplayNames;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DocumentFragment_488782e7a0312ee4: function(arg0) {
            let result;
            try {
                result = arg0 instanceof DocumentFragment;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Document_25f29ef741d52c29: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Document;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DomRectReadOnly_8166710734e9df85: function(arg0) {
            let result;
            try {
                result = arg0 instanceof DOMRectReadOnly;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DomRect_2079d319a06d2187: function(arg0) {
            let result;
            try {
                result = arg0 instanceof DOMRect;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DurationFormatOptions_b50c8fdccdaea0b9: function(arg0) {
            let result;
            try {
                result = arg0 instanceof DurationFormatOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DurationFormatPart_824505b46ba4e47e: function(arg0) {
            let result;
            try {
                result = arg0 instanceof DurationFormatPart;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_DurationFormat_fe8a40f8941b5360: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Intl.DurationFormat;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Duration_37c78ebbc297cd20: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Duration;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Element_73566cb5986eac3a: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Element;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ErrorEvent_87eddeb769532d5d: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ErrorEvent;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ErrorOptions_9d52b333e82d510a: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ErrorOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Error_94c8c9d9e410014a: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Error;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_EvalError_d4f118906e2e17cd: function(arg0) {
            let result;
            try {
                result = arg0 instanceof EvalError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_EventTarget_86847d800cdb77d2: function(arg0) {
            let result;
            try {
                result = arg0 instanceof EventTarget;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Event_54b0170b4e4224ac: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Event;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Exception_7805908b4a7089de: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WebAssembly.Exception;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_FinalizationRegistry_5a6936d4e3e08828: function(arg0) {
            let result;
            try {
                result = arg0 instanceof FinalizationRegistry;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Float16Array_25d1cffe420df643: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Float16Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Float32Array_826943b1e8c7500b: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Float32Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Float64Array_f5e58e6ef39e0d4a: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Float64Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Global_3ca9db56429ea77a: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WebAssembly.Global;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Global_a7a6dc87cbcdd71d: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Global;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_History_488d148b7ff9e1db: function(arg0) {
            let result;
            try {
                result = arg0 instanceof History;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlAnchorElement_4dd25f17aad759d0: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLAnchorElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlCanvasElement_8325b7578cc1684c: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLCanvasElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlElement_9d326f7a42217802: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlHeadElement_bfdadbfea5e00791: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLHeadElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlIFrameElement_306db1de42459221: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLIFrameElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlInputElement_684a7e5d7dbec24c: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLInputElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlStyleElement_a291de11ce477af2: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLStyleElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlTextAreaElement_ddb0b111033391f4: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLTextAreaElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Instance_183825903b24dce1: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WebAssembly.Instance;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Int16Array_5f78a4cd14a14269: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Int16Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Int32Array_1a28c9cdaf566d7d: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Int32Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Int8Array_598fa18f59afc16d: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Int8Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_JsClosure_be9d2e06d4fed2cb: function(arg0) {
            let result;
            try {
                result = arg0 instanceof JsClosure;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_JsString_89f8e91e0aed30d5: function(arg0) {
            let result;
            try {
                result = arg0 instanceof String;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_KeyboardEvent_2ecb44c131a7b1a6: function(arg0) {
            let result;
            try {
                result = arg0 instanceof KeyboardEvent;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_LinkError_3dc743649b08ec20: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WebAssembly.LinkError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ListFormatOptions_88be0ffcee20b1d8: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ListFormatOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ListFormatPart_70e9227d9953defa: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ListFormatPart;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ListFormat_43fc7e2cdf628890: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Intl.ListFormat;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_LocaleMatcherOptions_a39b0e375b60b281: function(arg0) {
            let result;
            try {
                result = arg0 instanceof LocaleMatcherOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Locale_de8384b7ece60482: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Intl.Locale;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Location_478fdb06ff3ef290: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Location;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_MaybeIterator_ec39104c0a28bb47: function(arg0) {
            let result;
            try {
                result = arg0 instanceof MaybeIterator;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_MediaQueryList_a845c65596b3067b: function(arg0) {
            let result;
            try {
                result = arg0 instanceof MediaQueryList;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Memory_4a1ccdf34cc1269b: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WebAssembly.Memory;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_MessageEvent_20df6b64aea8bf94: function(arg0) {
            let result;
            try {
                result = arg0 instanceof MessageEvent;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Module_4ee9f887ac5a51c2: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WebAssembly.Module;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_MouseEvent_bcc08a1c7e46e4a0: function(arg0) {
            let result;
            try {
                result = arg0 instanceof MouseEvent;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Navigator_0a92fd93a14da2ae: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Navigator;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Node_3149777213c50c67: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Node;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Null_14a2830ab5aa2c24: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Null;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_NumberFormatOptions_a0bf3e0cb5a83334: function(arg0) {
            let result;
            try {
                result = arg0 instanceof NumberFormatOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_NumberFormatPart_2afbc2ad443efb12: function(arg0) {
            let result;
            try {
                result = arg0 instanceof NumberFormatPart;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_NumberFormat_12a36fb6f2adcbf4: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Intl.NumberFormat;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_NumberRangeFormatPart_639b85cd570a9dcc: function(arg0) {
            let result;
            try {
                result = arg0 instanceof NumberRangeFormatPart;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Number_dd18df88a0869bdf: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Number;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Performance_1a12386577a93f55: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Performance;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_PluralRulesOptions_be9784897491b613: function(arg0) {
            let result;
            try {
                result = arg0 instanceof PluralRulesOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_PluralRules_d1c78901b13b7200: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Intl.PluralRules;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_PointerEvent_bbeecbc12422c20d: function(arg0) {
            let result;
            try {
                result = arg0 instanceof PointerEvent;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_PopStateEvent_80918dacd20774d4: function(arg0) {
            let result;
            try {
                result = arg0 instanceof PopStateEvent;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Proxy_b9d5f32e3c255fc9: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Proxy;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_RangeError_3e74ffc0e042baaa: function(arg0) {
            let result;
            try {
                result = arg0 instanceof RangeError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ReferenceError_01b1ffe95923acfe: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ReferenceError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_RegExpMatchArray_b9b61fa2f78f8702: function(arg0) {
            let result;
            try {
                result = arg0 instanceof RegExpMatchArray;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_RegExp_0fce680e85c54411: function(arg0) {
            let result;
            try {
                result = arg0 instanceof RegExp;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_RelativeTimeFormatOptions_7dc70a39c78fa094: function(arg0) {
            let result;
            try {
                result = arg0 instanceof RelativeTimeFormatOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_RelativeTimeFormatPart_8c7f3aee805f5fa3: function(arg0) {
            let result;
            try {
                result = arg0 instanceof RelativeTimeFormatPart;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_RelativeTimeFormat_177adb6857d0b473: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Intl.RelativeTimeFormat;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ResizeObserver_685713e6a57b3bcf: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ResizeObserver;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ResolvedCollatorOptions_50a9c5877cb80618: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ResolvedCollatorOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ResolvedDateTimeFormatOptions_0ad85142f197af74: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ResolvedDateTimeFormatOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ResolvedDisplayNamesOptions_bde4ec0440802d41: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ResolvedDisplayNamesOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ResolvedDurationFormatOptions_dff3cb5c4df96956: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ResolvedDurationFormatOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ResolvedListFormatOptions_a979b561733cbbda: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ResolvedListFormatOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ResolvedNumberFormatOptions_c914d4786e6de54e: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ResolvedNumberFormatOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ResolvedPluralRulesOptions_c4f2ccb06998b632: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ResolvedPluralRulesOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ResolvedRelativeTimeFormatOptions_b359a6f331557089: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ResolvedRelativeTimeFormatOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_ResolvedSegmenterOptions_d62829405f2e8ca4: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ResolvedSegmenterOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Response_cb984bd66d7bd408: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Response;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_RuntimeError_aa1728787ea28753: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WebAssembly.RuntimeError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_SegmentData_904c2959210a18f4: function(arg0) {
            let result;
            try {
                result = arg0 instanceof SegmentData;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_SegmenterOptions_690c9effab1af056: function(arg0) {
            let result;
            try {
                result = arg0 instanceof SegmenterOptions;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Segmenter_46905e6c2e812235: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Intl.Segmenter;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Segments_64f9dcac38b927b0: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Segments;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_SharedArrayBuffer_a4e3f0adbf3e8c9e: function(arg0) {
            let result;
            try {
                result = arg0 instanceof SharedArrayBuffer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_StyleSheet_96c991d9a5ca5858: function(arg0) {
            let result;
            try {
                result = arg0 instanceof StyleSheet;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Symbol_a729dab1f9adefa3: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Symbol;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_SyntaxError_e168f203d5d98348: function(arg0) {
            let result;
            try {
                result = arg0 instanceof SyntaxError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Table_e7fb3cbc0702f4c2: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WebAssembly.Table;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Tag_c72c70b26c848af5: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WebAssembly.Tag;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_TextInfo_9bbef55bba3041fe: function(arg0) {
            let result;
            try {
                result = arg0 instanceof TextInfo;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Text_0840046f927c03b6: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Text;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_TypeError_5ac1f87711e5ddc9: function(arg0) {
            let result;
            try {
                result = arg0 instanceof TypeError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_UiEvent_cb8f140a230236bb: function(arg0) {
            let result;
            try {
                result = arg0 instanceof UIEvent;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint16Array_850d0fde661a0393: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint16Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint32Array_55224f0204b6e661: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint32Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint8Array_86f30649f63ef9c2: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint8Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint8ClampedArray_a7c5bb45108f3d73: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint8ClampedArray;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Undefined_80c2f0cc71e7c049: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Undefined;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_UriError_4f4441130ba7ddea: function(arg0) {
            let result;
            try {
                result = arg0 instanceof URIError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Url_c9b2f07f2dfdad3c: function(arg0) {
            let result;
            try {
                result = arg0 instanceof URL;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_WebSocket_73af26207e88f1ac: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WebSocket;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_WeekInfo_8be67bc372a66499: function(arg0) {
            let result;
            try {
                result = arg0 instanceof WeekInfo;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Window_e093be59ee9a8e14: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Window;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instantiateStreaming_63ad9e1a2fd17dd5: function(arg0, arg1) {
            const ret = WebAssembly.instantiateStreaming(arg0, arg1);
            return ret;
        },
        __wbg_instantiate_72ffeba6a64a2062: function(arg0, arg1) {
            const ret = WebAssembly.instantiate(arg0, arg1);
            return ret;
        },
        __wbg_instantiate_7e13f144d7953ab2: function(arg0, arg1, arg2) {
            const ret = WebAssembly.instantiate(getArrayU8FromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_isArray_67c2c9c4313f4448: function(arg0) {
            const ret = Array.isArray(arg0);
            return ret;
        },
        __wbg_isArray_871ebcf4a2231067: function(arg0) {
            const ret = Array.isArray(arg0);
            return ret;
        },
        __wbg_isComposing_0b9f69114b4de822: function(arg0) {
            const ret = arg0.isComposing;
            return ret;
        },
        __wbg_isConcatSpreadable_da125f40ef18a49f: function() {
            const ret = Symbol.isConcatSpreadable;
            return ret;
        },
        __wbg_isConnected_55b350d5e0357104: function(arg0) {
            const ret = arg0.isConnected;
            return ret;
        },
        __wbg_isContentEditable_6fab7b2bae303f5b: function(arg0) {
            const ret = arg0.isContentEditable;
            return ret;
        },
        __wbg_isDefaultNamespace_976f7bd95caaed5b: function(arg0, arg1, arg2) {
            const ret = arg0.isDefaultNamespace(arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_isEqualNode_a7ee0d18cae3c6b5: function(arg0, arg1) {
            const ret = arg0.isEqualNode(arg1);
            return ret;
        },
        __wbg_isFinite_3f8c955d9065f4c5: function(arg0) {
            const ret = Number.isFinite(arg0);
            return ret;
        },
        __wbg_isFinite_6fac29bcb8c65adf: function(arg0) {
            const ret = isFinite(arg0);
            return ret;
        },
        __wbg_isInteger_2892b14ee5d82f0f: function(arg0) {
            const ret = Number.isInteger(arg0);
            return ret;
        },
        __wbg_isLockFree_5437ce18760cac3c: function(arg0) {
            const ret = Atomics.isLockFree(arg0 >>> 0);
            return ret;
        },
        __wbg_isNaN_7e4b7042447a1f14: function(arg0) {
            const ret = Number.isNaN(arg0);
            return ret;
        },
        __wbg_isPrimary_cd1e73d5446d0d3c: function(arg0) {
            const ret = arg0.isPrimary;
            return ret;
        },
        __wbg_isSafeInteger_66acec27e09e99a7: function(arg0) {
            const ret = Number.isSafeInteger(arg0);
            return ret;
        },
        __wbg_isSameNode_3e3f9933a353d732: function(arg0, arg1) {
            const ret = arg0.isSameNode(arg1);
            return ret;
        },
        __wbg_isSecureContext_ef5e466438f051c0: function(arg0) {
            const ret = arg0.isSecureContext;
            return ret;
        },
        __wbg_isTrusted_0becfdeaaba9765f: function(arg0) {
            const ret = arg0.isTrusted;
            return ret;
        },
        __wbg_isView_7196124a1d67a99a: function(arg0) {
            const ret = ArrayBuffer.isView(arg0);
            return ret;
        },
        __wbg_is_4801976d24bcae5b: function(arg0, arg1) {
            const ret = Object.is(arg0, arg1);
            return ret;
        },
        __wbg_is_860b01f7cd3f2ec7: function(arg0, arg1) {
            const ret = arg0.is(arg1);
            return ret;
        },
        __wbg_is_word_like_2dec617afb5c0b97: function(arg0) {
            const ret = arg0.isWordLike;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg_item_8a2cd340951df6f4: function(arg0, arg1) {
            const ret = arg0.item(arg1 >>> 0);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_item_bcd019a7bf402aad: function(arg0, arg1, arg2) {
            const ret = arg1.item(arg2 >>> 0);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_iterator_8732428d309e270e: function() {
            const ret = Symbol.iterator;
            return ret;
        },
        __wbg_json_8b1fa0ed507324c9: function() { return handleError(function (arg0) {
            const ret = arg0.json();
            return ret;
        }, arguments); },
        __wbg_keyCode_a62e266ec732401d: function(arg0) {
            const ret = arg0.keyCode;
            return ret;
        },
        __wbg_keyFor_cf419efb02fa5757: function(arg0) {
            const ret = Symbol.keyFor(arg0);
            return ret;
        },
        __wbg_key_df6a54e3e036c3fe: function(arg0, arg1) {
            const ret = arg1.key;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_lang_f3d1a3c5b93354ed: function(arg0, arg1) {
            const ret = arg1.lang;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_language_537b3952532c33e6: function(arg0, arg1) {
            const ret = arg1.language;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_language_ad2fb82474516e48: function(arg0) {
            const ret = arg0.language;
            return ret;
        },
        __wbg_languages_4159591877e66d56: function(arg0) {
            const ret = arg0.languages;
            return ret;
        },
        __wbg_lastChild_db5fd7b4280df4ee: function(arg0) {
            const ret = arg0.lastChild;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_lastElementChild_3b99b93f19644088: function(arg0) {
            const ret = arg0.lastElementChild;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_lastElementChild_59d26c63dada02d2: function(arg0) {
            const ret = arg0.lastElementChild;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_lastElementChild_b2b6617c2e43af00: function(arg0) {
            const ret = arg0.lastElementChild;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_lastEventId_cea418b2c38bc78f: function(arg0, arg1) {
            const ret = arg1.lastEventId;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_lastIndexOf_9c1fd0f41f0d9789: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.lastIndexOf(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        },
        __wbg_lastMatch_e127d176ffcd33f3: function() {
            const ret = RegExp.lastMatch;
            return ret;
        },
        __wbg_lastModified_1624f37c5ca1aa71: function(arg0, arg1) {
            const ret = arg1.lastModified;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_lastParen_cff7cd4b2a773398: function() {
            const ret = RegExp.lastParen;
            return ret;
        },
        __wbg_lastStyleSheetSet_59a5ad1be9238009: function(arg0, arg1) {
            const ret = arg1.lastStyleSheetSet;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_last_index_0d5dd63ad7e63f32: function(arg0) {
            const ret = arg0.lastIndex;
            return ret;
        },
        __wbg_layerX_b207f5c056a4ab77: function(arg0) {
            const ret = arg0.layerX;
            return ret;
        },
        __wbg_layerY_0cf336a342a20252: function(arg0) {
            const ret = arg0.layerY;
            return ret;
        },
        __wbg_leftContext_94404b65388bd635: function() {
            const ret = RegExp.leftContext;
            return ret;
        },
        __wbg_left_985fa07b897d8e74: function(arg0) {
            const ret = arg0.left;
            return ret;
        },
        __wbg_length_0c327b454773d0a2: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_208bf85f3934fbb8: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_27cfc5149a8eb2e7: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_280688879ee7deb5: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_29382b6304954bd0: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_33096ac1966bb961: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_4a591ecaa01354d9: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_521edf73d0cf740d: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_5c5481313a7ae148: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_6b04cfc608031423: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_6ba56c4cbd6caf2f: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_7abca14930109c1c: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_85f11d57c87aab8f: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_8d7d601abee5b961: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_a77ff6a94e0f2e1a: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_b2624494d18202da: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_b45bdc920bf932d6: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_bc3cf78f4f0ddb59: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_c0c6c8eeee5aeea6: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_c4e3c9d78d534191: function() { return handleError(function (arg0) {
            const ret = arg0.length;
            return ret;
        }, arguments); },
        __wbg_lineno_f63bab95283a7005: function(arg0) {
            const ret = arg0.lineno;
            return ret;
        },
        __wbg_list_cd703919e2cf04fe: function(arg0) {
            const ret = arg0.list;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_localName_d573a6949381eccf: function(arg0, arg1) {
            const ret = arg1.localName;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_localeCompare_b4f18927a8a28870: function(arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.localeCompare(getStringFromWasm0(arg1, arg2), arg3, arg4);
            return ret;
        },
        __wbg_location_5357c7b9bb26d987: function(arg0) {
            const ret = arg0.location;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_location_bd39406d76c6d592: function(arg0) {
            const ret = arg0.location;
            return ret;
        },
        __wbg_location_efdf1fea18b5552a: function(arg0) {
            const ret = arg0.location;
            return ret;
        },
        __wbg_log10_b3cbdad4265493cf: function(arg0) {
            const ret = Math.log10(arg0);
            return ret;
        },
        __wbg_log1p_5d53b81ba287dd37: function(arg0) {
            const ret = Math.log1p(arg0);
            return ret;
        },
        __wbg_log2_8eb4370d4724a46d: function(arg0) {
            const ret = Math.log2(arg0);
            return ret;
        },
        __wbg_log_28cb5f9435b686fa: function(arg0) {
            const ret = Math.log(arg0);
            return ret;
        },
        __wbg_log_52235c3ee9f80b80: function() {
            console.log();
        },
        __wbg_log_98b21c23b7d2f47f: function(arg0) {
            console.log(...arg0);
        },
        __wbg_log_a8aef091252f373d: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.log(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_log_c5d6fe09a7a1f44c: function(arg0, arg1, arg2, arg3, arg4) {
            console.log(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_log_cf2e968649f3384e: function(arg0) {
            console.log(arg0);
        },
        __wbg_log_d56195e92218e087: function(arg0, arg1) {
            console.log(arg0, arg1);
        },
        __wbg_log_d5e0f90a3ac097e3: function(arg0, arg1, arg2, arg3) {
            console.log(arg0, arg1, arg2, arg3);
        },
        __wbg_log_df1a5b4cbce8881f: function(arg0, arg1, arg2) {
            console.log(arg0, arg1, arg2);
        },
        __wbg_log_e8fe3c953227a729: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.log(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_longDesc_8e268422e40097cc: function(arg0, arg1) {
            const ret = arg1.longDesc;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_lookupNamespaceURI_2da8be06a05d3ff1: function(arg0, arg1, arg2, arg3) {
            const ret = arg1.lookupNamespaceURI(arg2 === 0 ? undefined : getStringFromWasm0(arg2, arg3));
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_lookupPrefix_36755678be43fa3d: function(arg0, arg1, arg2, arg3) {
            const ret = arg1.lookupPrefix(arg2 === 0 ? undefined : getStringFromWasm0(arg2, arg3));
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_marginHeight_3aa6c79d38b36b81: function(arg0, arg1) {
            const ret = arg1.marginHeight;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_marginWidth_051da048a38d4d3b: function(arg0, arg1) {
            const ret = arg1.marginWidth;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_mark_a6f18925d4b42a05: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.mark(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_matchAll_1747b8e891590381: function(arg0, arg1) {
            const ret = arg0.matchAll(arg1);
            return ret;
        },
        __wbg_matchMedia_072e51dea8ac78bd: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.matchMedia(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_match_0b17dd5183757d3e: function(arg0, arg1) {
            const ret = arg0.match(arg1);
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_match_a58b9188f1d3d350: function() {
            const ret = Symbol.match;
            return ret;
        },
        __wbg_matches_2e67d4c454e23c6b: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.matches(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_matches_2f598e82be2a8afc: function(arg0) {
            const ret = arg0.matches;
            return ret;
        },
        __wbg_maxByteLength_5d8fbd4f5b2a8bce: function(arg0) {
            const ret = arg0.maxByteLength;
            return ret;
        },
        __wbg_maxByteLength_883a233ad8a52636: function(arg0) {
            const ret = arg0.maxByteLength;
            return ret;
        },
        __wbg_maxByteLength_b913371a1e05e6b2: function(arg0) {
            const ret = arg0.maxByteLength;
            return ret;
        },
        __wbg_maxLength_59f18da969fd0097: function(arg0) {
            const ret = arg0.maxLength;
            return ret;
        },
        __wbg_maxLength_af29e418f361f644: function(arg0) {
            const ret = arg0.maxLength;
            return ret;
        },
        __wbg_maxTouchPoints_7a09d1aea47eab3d: function(arg0) {
            const ret = arg0.maxTouchPoints;
            return ret;
        },
        __wbg_max_9a329243e12423d6: function(arg0, arg1) {
            const ret = arg1.max;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_max_d4d061a678767b7b: function(arg0, arg1) {
            const ret = Math.max(arg0, arg1);
            return ret;
        },
        __wbg_maximize_26b4e52cf18ad9e6: function(arg0) {
            const ret = arg0.maximize();
            return ret;
        },
        __wbg_measure_1fdaf89c740a34a1: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.measure(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_measure_6bdb441e10e46a66: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.measure(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_measure_d17f63c9b3ae0501: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.measure(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_media_43e4fbd91021075a: function(arg0, arg1) {
            const ret = arg1.media;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_media_9e51b5788a577c00: function(arg0, arg1) {
            const ret = arg1.media;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_message_40300ed2d1f8bdc6: function(arg0) {
            const ret = arg0.message;
            return ret;
        },
        __wbg_message_ab75609e36338e7c: function(arg0, arg1) {
            const ret = arg1.message;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_metaKey_752862905c708ca9: function(arg0) {
            const ret = arg0.metaKey;
            return ret;
        },
        __wbg_metaKey_d2a47aa621ff2c45: function(arg0) {
            const ret = arg0.metaKey;
            return ret;
        },
        __wbg_microseconds_03436dbffefd9b3c: function(arg0, arg1) {
            const ret = arg1.microseconds;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_milliseconds_18112df0af3567f6: function(arg0, arg1) {
            const ret = arg1.milliseconds;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_minLength_41cf6267161dcc9b: function(arg0) {
            const ret = arg0.minLength;
            return ret;
        },
        __wbg_minLength_bbe2659e3f513d5b: function(arg0) {
            const ret = arg0.minLength;
            return ret;
        },
        __wbg_min_ccb676af03183271: function(arg0, arg1) {
            const ret = Math.min(arg0, arg1);
            return ret;
        },
        __wbg_min_f6e7b3c91871282c: function(arg0, arg1) {
            const ret = arg1.min;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_minimal_days_c8168b6c9897b8fb: function(arg0) {
            const ret = arg0.minimalDays;
            return ret;
        },
        __wbg_minimize_8affdcfac6d038a1: function(arg0) {
            const ret = arg0.minimize();
            return ret;
        },
        __wbg_minutes_83bd7e8732f4df06: function(arg0, arg1) {
            const ret = arg1.minutes;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_months_93ae3028663caa75: function(arg0, arg1) {
            const ret = arg1.months;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_moveBy_be527cff02bce6c7: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.moveBy(arg1, arg2);
        }, arguments); },
        __wbg_moveTo_6359b9b91798b21f: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.moveTo(arg1, arg2);
        }, arguments); },
        __wbg_movementX_41f36d601d93b52a: function(arg0) {
            const ret = arg0.movementX;
            return ret;
        },
        __wbg_movementY_24e4e049b09f7449: function(arg0) {
            const ret = arg0.movementY;
            return ret;
        },
        __wbg_multiline_44e4aa3cfb5b79ca: function(arg0) {
            const ret = arg0.multiline;
            return ret;
        },
        __wbg_multiple_0703e7902699ac24: function(arg0) {
            const ret = arg0.multiple;
            return ret;
        },
        __wbg_name_1e38f6c25b84ad53: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_name_235d35583f1ac035: function(arg0, arg1) {
            const ret = arg1.name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_name_2e3d97e5d7abee6d: function(arg0) {
            const ret = arg0.name;
            return ret;
        },
        __wbg_name_33b65b88005c2c78: function(arg0, arg1) {
            const ret = arg1.name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_name_6146195d9c04814d: function(arg0, arg1) {
            const ret = arg1.name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_name_f16d012782a1f99b: function(arg0, arg1) {
            const ret = arg1.name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_namespaceURI_54e3421b18686940: function(arg0, arg1) {
            const ret = arg1.namespaceURI;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_nanoseconds_c1c3dcb221909d86: function(arg0, arg1) {
            const ret = arg1.nanoseconds;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_navigator_3833ecdbc19d2757: function(arg0) {
            const ret = arg0.navigator;
            return ret;
        },
        __wbg_new_03a9d630f327843a: function(arg0, arg1) {
            const ret = new SyntaxError(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_05e5a54b1d79320b: function(arg0) {
            const ret = new BigUint64Array(arg0);
            return ret;
        },
        __wbg_new_081429c5b2286353: function(arg0, arg1) {
            const ret = new Intl.RelativeTimeFormat(arg0, arg1);
            return ret;
        },
        __wbg_new_0_445c13a750296eb6: function() {
            const ret = new Date();
            return ret;
        },
        __wbg_new_0f3ffe2f5b3aa624: function(arg0) {
            const ret = new Uint8ClampedArray(arg0);
            return ret;
        },
        __wbg_new_0f71502e7f007644: function() { return handleError(function (arg0) {
            const ret = new WebAssembly.Module(arg0);
            return ret;
        }, arguments); },
        __wbg_new_0ffa3e7afb4459a9: function(arg0) {
            const ret = new Int16Array(arg0);
            return ret;
        },
        __wbg_new_13de3a691a0a2540: function() { return handleError(function () {
            const ret = new DOMRectReadOnly();
            return ret;
        }, arguments); },
        __wbg_new_167dfad0e50f0a3b: function(arg0, arg1) {
            const ret = new TypeError(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_17a2c02b89c37aa4: function(arg0, arg1) {
            const ret = new Intl.DateTimeFormat(arg0, arg1);
            return ret;
        },
        __wbg_new_1b728150e61a2d25: function() { return handleError(function (arg0, arg1) {
            const ret = new ErrorEvent(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_25d0792e7e399976: function(arg0) {
            const ret = new Uint32Array(arg0);
            return ret;
        },
        __wbg_new_280ee4612cba9f98: function(arg0, arg1) {
            const ret = new Intl.Collator(arg0, arg1);
            return ret;
        },
        __wbg_new_2abfd00d5a4bdb08: function() { return handleError(function () {
            const ret = new Response();
            return ret;
        }, arguments); },
        __wbg_new_2c48d7fdccf94f7a: function(arg0) {
            const ret = new Float32Array(arg0);
            return ret;
        },
        __wbg_new_30c24bdc8f80f1a5: function(arg0) {
            const ret = new Boolean(arg0);
            return ret;
        },
        __wbg_new_31ac8633a73459af: function() { return handleError(function (arg0) {
            const ret = new ResizeObserver(arg0);
            return ret;
        }, arguments); },
        __wbg_new_330bbd466f3f19df: function() { return handleError(function (arg0, arg1) {
            const ret = new CloseEvent(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_379810623de1c33e: function(arg0, arg1) {
            const ret = new EvalError(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_450180f1d60fe343: function() { return handleError(function (arg0) {
            const ret = new WebAssembly.Memory(arg0);
            return ret;
        }, arguments); },
        __wbg_new_45fe4cc5d062e855: function(arg0, arg1) {
            const ret = new Intl.NumberFormat(arg0, arg1);
            return ret;
        },
        __wbg_new_47c9ec7c77a03467: function() { return handleError(function (arg0) {
            const ret = new WebAssembly.Table(arg0);
            return ret;
        }, arguments); },
        __wbg_new_48a4bb9e2b081c58: function(arg0, arg1) {
            const ret = new Intl.PluralRules(arg0, arg1);
            return ret;
        },
        __wbg_new_4b4e6a0ba5ccc584: function(arg0, arg1) {
            const ret = new WebAssembly.CompileError(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_4bb56673377012f4: function() { return handleError(function () {
            const ret = new EventTarget();
            return ret;
        }, arguments); },
        __wbg_new_4c777fd322e0d83c: function() { return handleError(function (arg0, arg1) {
            const ret = new MouseEvent(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_4ef7d98dc9bf4ac5: function(arg0, arg1) {
            const ret = new RangeError(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_4fa35fe5ad5b9769: function() { return handleError(function (arg0, arg1) {
            const ret = new MessageEvent(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_50bb5ebeecef71a8: function(arg0, arg1) {
            const ret = new Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_578aeef4b6b94378: function(arg0) {
            const ret = new Uint8Array(arg0);
            return ret;
        },
        __wbg_new_5cd566492f450ae1: function(arg0, arg1) {
            const ret = new WebAssembly.RuntimeError(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_622fc80556be2e26: function() {
            const ret = new Map();
            return ret;
        },
        __wbg_new_623eb031e23ce92c: function() { return handleError(function (arg0, arg1) {
            const ret = new UIEvent(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_64c66d807d94ee2f: function(arg0) {
            const ret = new Set(arg0);
            return ret;
        },
        __wbg_new_687e6e45fb06b228: function() {
            const ret = new WeakMap();
            return ret;
        },
        __wbg_new_69240efb3ceaa732: function(arg0) {
            const ret = new FinalizationRegistry(arg0);
            return ret;
        },
        __wbg_new_6d75fd236f920a62: function(arg0) {
            const ret = new Date(arg0);
            return ret;
        },
        __wbg_new_6f440f95074df60f: function() { return handleError(function (arg0, arg1) {
            const ret = new PointerEvent(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_78bba7c62accb73d: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = new Intl.DurationFormat(getArrayJsValueViewFromWasm0(arg0, arg1), arg2);
            return ret;
        }, arguments); },
        __wbg_new_78d96fc6eea6da7a: function() { return handleError(function (arg0, arg1) {
            const ret = new Event(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_81dc094f0d3490c4: function(arg0, arg1, arg2, arg3) {
            const ret = new RegExp(getStringFromWasm0(arg0, arg1), getStringFromWasm0(arg2, arg3));
            return ret;
        },
        __wbg_new_8d67bd740527e37f: function(arg0) {
            const ret = new Float64Array(arg0);
            return ret;
        },
        __wbg_new_91dc6e5dc21bb50d: function() { return handleError(function (arg0, arg1) {
            const ret = new PopStateEvent(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_953798eff1844040: function(arg0) {
            const ret = new Number(arg0);
            return ret;
        },
        __wbg_new_9ad739c0079fd150: function() { return handleError(function () {
            const ret = new Document();
            return ret;
        }, arguments); },
        __wbg_new_9b4a7350c3a4e0ba: function() { return handleError(function () {
            const ret = new Text();
            return ret;
        }, arguments); },
        __wbg_new_9d237684a71be5e6: function(arg0, arg1) {
            const ret = new Intl.DisplayNames(arg0, arg1);
            return ret;
        },
        __wbg_new_9df55c460bb29f45: function() { return handleError(function (arg0, arg1) {
            const ret = new KeyboardEvent(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_9f114d5280d30bd4: function(arg0, arg1) {
            const ret = new WebAssembly.LinkError(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_9f93e880f5a00f2f: function() { return handleError(function () {
            const ret = new Blob();
            return ret;
        }, arguments); },
        __wbg_new_a1159b37445b24d4: function(arg0, arg1) {
            const ret = new Intl.Segmenter(arg0, arg1);
            return ret;
        },
        __wbg_new_b17805b720db06ae: function() { return handleError(function (arg0, arg1) {
            const ret = new WebAssembly.Instance(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_new_b733f187b7a8b317: function() { return handleError(function (arg0, arg1) {
            const ret = new URL(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_baaedf9edc130b80: function(arg0, arg1) {
            const ret = new Proxy(arg0, arg1);
            return ret;
        },
        __wbg_new_c2afe36052bfe7be: function(arg0) {
            const ret = new BigInt64Array(arg0);
            return ret;
        },
        __wbg_new_c542c57f4a5d81e7: function(arg0) {
            const ret = new Uint16Array(arg0);
            return ret;
        },
        __wbg_new_cba56cba9b8c261b: function() {
            const ret = new WeakSet();
            return ret;
        },
        __wbg_new_ce1ab61c1c2b300d: function() {
            const ret = new Object();
            return ret;
        },
        __wbg_new_d14d200e319fb83d: function(arg0) {
            const ret = new Float16Array(arg0);
            return ret;
        },
        __wbg_new_d35aa408f4bf4aa1: function(arg0) {
            const ret = new ArrayBuffer(arg0 >>> 0);
            return ret;
        },
        __wbg_new_d4858e8447d97719: function(arg0, arg1) {
            const ret = new Intl.ListFormat(arg0, arg1);
            return ret;
        },
        __wbg_new_d4c752f1281b2f74: function(arg0) {
            const ret = new SharedArrayBuffer(arg0 >>> 0);
            return ret;
        },
        __wbg_new_d72dad8fe183fb2c: function(arg0, arg1, arg2) {
            const ret = new DataView(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_d7e476b433a26bea: function() { return handleError(function (arg0, arg1) {
            const ret = new WebSocket(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_d90091b82fdf5b91: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_dc32d91df76232c8: function(arg0) {
            const ret = new Int32Array(arg0);
            return ret;
        },
        __wbg_new_ded7943cd19efb28: function() { return handleError(function () {
            const ret = new DocumentFragment();
            return ret;
        }, arguments); },
        __wbg_new_df45100e9427b22d: function() { return handleError(function (arg0, arg1) {
            const ret = new Intl.Locale(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_e00233b545e44c89: function() { return handleError(function (arg0, arg1) {
            const ret = new WebAssembly.Global(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_new_e02663761daeb2a3: function() { return handleError(function () {
            const ret = new CSSStyleSheet();
            return ret;
        }, arguments); },
        __wbg_new_e08f07090841233c: function() { return handleError(function () {
            const ret = new DOMRect();
            return ret;
        }, arguments); },
        __wbg_new_ecb83b64746323ee: function(arg0, arg1) {
            const ret = new URIError(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_eefa07e4e44bba37: function(arg0, arg1) {
            const ret = new ReferenceError(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_ef535c653415827c: function() { return handleError(function (arg0) {
            const ret = new WebAssembly.Tag(arg0);
            return ret;
        }, arguments); },
        __wbg_new_f49b8efd58c31e07: function(arg0, arg1) {
            const ret = new AggregateError(getArrayJsValueViewFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_f6d9cadfaf66afc1: function() { return handleError(function (arg0, arg1) {
            const ret = new WebAssembly.Exception(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_new_fffed363013eb30a: function(arg0) {
            const ret = new Int8Array(arg0);
            return ret;
        },
        __wbg_new_from_slice_18fa1f71286d66b8: function(arg0, arg1) {
            const ret = new Uint8Array(getArrayU8FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_1b23616ada25abcb: function(arg0, arg1) {
            const ret = new Int32Array(getArrayI32FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_332b97e8c7a5c7e4: function(arg0, arg1) {
            const ret = new Uint8ClampedArray(getArrayU8FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_3c93d0bc613de8f0: function(arg0, arg1) {
            const ret = new Float64Array(getArrayF64FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_47be4219028de35d: function(arg0, arg1) {
            const ret = new Uint32Array(getArrayU32FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_6c21d47c5acff32d: function(arg0, arg1) {
            const ret = new BigUint64Array(getArrayU64FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_74ab638f08c2c100: function(arg0, arg1) {
            const ret = new Int16Array(getArrayI16FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_956df4f769fb782c: function(arg0, arg1) {
            const ret = new Float32Array(getArrayF32FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_a0009695698d1671: function(arg0, arg1) {
            const ret = new Uint16Array(getArrayU16FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_d2708ef7f913ec0e: function(arg0, arg1) {
            const ret = new BigInt64Array(getArrayI64FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_e26742c9dfcc6c2d: function(arg0, arg1) {
            const ret = new Int8Array(getArrayI8FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_str_691f8248c06931a5: function(arg0, arg1) {
            const ret = new Number(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_no_args_67747f9e48367a42: function(arg0, arg1) {
            const ret = new Function(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_regexp_fdfbb63934867fb0: function(arg0, arg1, arg2) {
            const ret = new RegExp(arg0, getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_new_typed_0cb2f351421aa439: function() {
            const ret = new Set();
            return ret;
        },
        __wbg_new_with_args_b7569746028c197c: function(arg0, arg1, arg2, arg3) {
            const ret = new Function(getStringFromWasm0(arg0, arg1), getStringFromWasm0(arg2, arg3));
            return ret;
        },
        __wbg_new_with_base_738bee8a96b1b249: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = new URL(getStringFromWasm0(arg0, arg1), getStringFromWasm0(arg2, arg3));
            return ret;
        }, arguments); },
        __wbg_new_with_blob_sequence_and_options_bc65602d04f72c74: function() { return handleError(function (arg0, arg1) {
            const ret = new Blob(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_new_with_blob_sequence_e85f53f12904bb25: function() { return handleError(function (arg0) {
            const ret = new Blob(arg0);
            return ret;
        }, arguments); },
        __wbg_new_with_buffer_source_sequence_451fb403517240b4: function() { return handleError(function (arg0) {
            const ret = new Blob(arg0);
            return ret;
        }, arguments); },
        __wbg_new_with_buffer_source_sequence_and_options_c6f4cbbc74677636: function() { return handleError(function (arg0, arg1) {
            const ret = new Blob(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_new_with_byte_offset_1f9bcd8feaf8552e: function(arg0, arg1) {
            const ret = new Int32Array(arg0, arg1 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_1feeb05c95d14036: function(arg0, arg1) {
            const ret = new Uint8Array(arg0, arg1 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_3ad4bce1e9687f75: function(arg0, arg1) {
            const ret = new Uint8ClampedArray(arg0, arg1 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_4a5c928a508d16d6: function(arg0, arg1) {
            const ret = new Float16Array(arg0, arg1 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_64450ccc6462a9e6: function(arg0, arg1) {
            const ret = new Float64Array(arg0, arg1 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_682679a377eb6d14: function(arg0, arg1) {
            const ret = new BigUint64Array(arg0, arg1 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_7f477f11aca9ad05: function(arg0, arg1) {
            const ret = new Int16Array(arg0, arg1 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_88f7f086789e0201: function(arg0, arg1) {
            const ret = new Float32Array(arg0, arg1 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_a2e98240d97ae973: function(arg0, arg1) {
            const ret = new Uint32Array(arg0, arg1 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_a518a940b9382151: function(arg0, arg1) {
            const ret = new Uint16Array(arg0, arg1 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_a9dc711cb606442a: function(arg0, arg1) {
            const ret = new BigInt64Array(arg0, arg1 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_and_length_017f12ab16676997: function(arg0, arg1, arg2) {
            const ret = new BigInt64Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_and_length_281228aa9c9441ef: function(arg0, arg1, arg2) {
            const ret = new Uint32Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_and_length_38cea5bf82674a3a: function(arg0, arg1, arg2) {
            const ret = new Int32Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_and_length_49f6916db1dd9671: function(arg0, arg1, arg2) {
            const ret = new Float16Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_and_length_5e93da92e88bdbb7: function(arg0, arg1, arg2) {
            const ret = new Int8Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_and_length_69100802f82ea5a5: function(arg0, arg1, arg2) {
            const ret = new Float64Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_and_length_99f582c9200a6594: function(arg0, arg1, arg2) {
            const ret = new Uint16Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_and_length_ae1b31d177f9a074: function(arg0, arg1, arg2) {
            const ret = new Float32Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_and_length_d836f26d916dd9ad: function(arg0, arg1, arg2) {
            const ret = new Uint8Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_and_length_dd735bca0bc99847: function(arg0, arg1, arg2) {
            const ret = new Uint8ClampedArray(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_and_length_dfba28d4e12c0c91: function(arg0, arg1, arg2) {
            const ret = new BigUint64Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_and_length_f85909291ba1c227: function(arg0, arg1, arg2) {
            const ret = new Int16Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_byte_offset_b9612ff553d1c72b: function(arg0, arg1) {
            const ret = new Int8Array(arg0, arg1 >>> 0);
            return ret;
        },
        __wbg_new_with_data_a02dc7a61915a2eb: function() { return handleError(function (arg0, arg1) {
            const ret = new Text(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_with_error_options_b1fb191e7a43d416: function(arg0, arg1, arg2) {
            const ret = new Error(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_new_with_length_1eb3643ef8ff666a: function(arg0) {
            const ret = new Int32Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_length_21806dff02ad1c3e: function(arg0) {
            const ret = new Uint32Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_length_36a4998e27b014c5: function(arg0) {
            const ret = new Uint8Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_length_48a9c83b8a87d11f: function(arg0) {
            const ret = new BigInt64Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_length_4bcd86182574c707: function(arg0) {
            const ret = new BigUint64Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_length_5a54328420579213: function(arg0) {
            const ret = new Int8Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_length_690552eb9e6aeac9: function(arg0) {
            const ret = new Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_length_7d20818cf1afe359: function(arg0) {
            const ret = new Float32Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_length_8f9278dbad0d5670: function(arg0) {
            const ret = new Uint16Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_length_9ac6a9917b8558cb: function(arg0) {
            const ret = new Int16Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_length_a6c7793a7ae55c1c: function(arg0) {
            const ret = new Float16Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_length_a8e81ca9a3d0cfa0: function(arg0) {
            const ret = new Uint8ClampedArray(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_length_b4a87ccced374381: function(arg0) {
            const ret = new Float64Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_message_24ff3f255e7de6e1: function(arg0, arg1, arg2, arg3) {
            const ret = new AggregateError(getArrayJsValueViewFromWasm0(arg0, arg1), getStringFromWasm0(arg2, arg3));
            return ret;
        },
        __wbg_new_with_opt_blob_583fa4e8588b70ac: function() { return handleError(function (arg0) {
            const ret = new Response(arg0);
            return ret;
        }, arguments); },
        __wbg_new_with_opt_buffer_source_5c3567648ece2768: function() { return handleError(function (arg0) {
            const ret = new Response(arg0);
            return ret;
        }, arguments); },
        __wbg_new_with_opt_js_u8_array_2a4049ccb680ed99: function() { return handleError(function (arg0) {
            const ret = new Response(arg0);
            return ret;
        }, arguments); },
        __wbg_new_with_opt_str_2fdd334a0ee65f5d: function() { return handleError(function (arg0, arg1) {
            const ret = new Response(arg0 === 0 ? undefined : getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_with_opt_u8_array_56d3d414286a951c: function() { return handleError(function (arg0, arg1) {
            const ret = new Response(arg0 === 0 ? undefined : getArrayU8FromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_new_with_options_0830128dede5cdea: function(arg0, arg1, arg2) {
            const ret = new TypeError(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_new_with_options_14dea7f5d9dcc231: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = new WebAssembly.Exception(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_new_with_options_449f1e7132b91d0d: function(arg0, arg1, arg2) {
            const ret = new Error(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_new_with_options_5d702244106e45e9: function(arg0, arg1, arg2, arg3, arg4) {
            const ret = new AggregateError(getArrayJsValueViewFromWasm0(arg0, arg1), getStringFromWasm0(arg2, arg3), arg4);
            return ret;
        },
        __wbg_new_with_options_5e0cf257af8853d8: function(arg0, arg1, arg2) {
            const ret = new WebAssembly.CompileError(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_new_with_options_6584ce736c988d9a: function(arg0, arg1, arg2) {
            const ret = new SyntaxError(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_new_with_options_730d31e22dee0d7a: function(arg0, arg1, arg2) {
            const ret = new EvalError(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_new_with_options_a86cedbaadc7e754: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = new Intl.Locale(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        }, arguments); },
        __wbg_new_with_options_d44cd5764711fb3c: function(arg0, arg1, arg2) {
            const ret = new WebAssembly.RuntimeError(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_new_with_options_d63676243d58fe3d: function(arg0, arg1, arg2) {
            const ret = new RangeError(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_new_with_options_e6babf9880d81383: function(arg0, arg1, arg2) {
            const ret = new WebAssembly.LinkError(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_new_with_options_e91cd3d37bd66a74: function(arg0, arg1, arg2) {
            const ret = new ReferenceError(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_new_with_options_f37ad0ee41ffd77f: function(arg0, arg1, arg2) {
            const ret = new URIError(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_new_with_options_f51714c34b71ada6: function(arg0, arg1) {
            const ret = new SharedArrayBuffer(arg0 >>> 0, arg1);
            return ret;
        },
        __wbg_new_with_options_fa2d4e2f4b375f3d: function(arg0, arg1) {
            const ret = new ArrayBuffer(arg0 >>> 0, arg1);
            return ret;
        },
        __wbg_new_with_shared_array_buffer_91c25371e65e90d7: function(arg0, arg1, arg2) {
            const ret = new DataView(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_str_008af93d6d4c05a6: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = new WebSocket(getStringFromWasm0(arg0, arg1), getStringFromWasm0(arg2, arg3));
            return ret;
        }, arguments); },
        __wbg_new_with_str_sequence_9ed2327430efed8d: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = new WebSocket(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        }, arguments); },
        __wbg_new_with_str_sequence_and_options_21cfcd771283a47d: function() { return handleError(function (arg0, arg1) {
            const ret = new Blob(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_new_with_str_sequence_eabf85a6027f8970: function() { return handleError(function (arg0) {
            const ret = new Blob(arg0);
            return ret;
        }, arguments); },
        __wbg_new_with_u8_array_sequence_72c2afc92d790182: function() { return handleError(function (arg0) {
            const ret = new Blob(arg0);
            return ret;
        }, arguments); },
        __wbg_new_with_u8_array_sequence_and_options_99c158c319b91878: function() { return handleError(function (arg0, arg1) {
            const ret = new Blob(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_new_with_u8_slice_sequence_982bd2f3aa9cc452: function() { return handleError(function (arg0) {
            const ret = new Blob(arg0);
            return ret;
        }, arguments); },
        __wbg_new_with_u8_slice_sequence_and_options_9e1aff149489822c: function() { return handleError(function (arg0, arg1) {
            const ret = new Blob(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_new_with_value_d4e05f1cac30971e: function() { return handleError(function (arg0, arg1) {
            const ret = new WebAssembly.Table(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_new_with_x_399ae8edbc4326db: function() { return handleError(function (arg0) {
            const ret = new DOMRectReadOnly(arg0);
            return ret;
        }, arguments); },
        __wbg_new_with_x_and_y_288ecce8ed54537a: function() { return handleError(function (arg0, arg1) {
            const ret = new DOMRect(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_new_with_x_and_y_941ebb7b3de2bcae: function() { return handleError(function (arg0, arg1) {
            const ret = new DOMRectReadOnly(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_new_with_x_and_y_and_width_and_height_03459540779148a9: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = new DOMRect(arg0, arg1, arg2, arg3);
            return ret;
        }, arguments); },
        __wbg_new_with_x_and_y_and_width_and_height_078168ff9294d43c: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = new DOMRectReadOnly(arg0, arg1, arg2, arg3);
            return ret;
        }, arguments); },
        __wbg_new_with_x_and_y_and_width_ca97c2c998d98869: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = new DOMRectReadOnly(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_new_with_x_and_y_and_width_e57ffc317afa10b8: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = new DOMRect(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_new_with_x_e263df67014b4f29: function() { return handleError(function (arg0) {
            const ret = new DOMRect(arg0);
            return ret;
        }, arguments); },
        __wbg_new_with_year_month_2cee052c4420ec44: function(arg0, arg1) {
            const ret = new Date(arg0 >>> 0, arg1);
            return ret;
        },
        __wbg_new_with_year_month_day_1c037825a6927e19: function(arg0, arg1, arg2) {
            const ret = new Date(arg0 >>> 0, arg1, arg2);
            return ret;
        },
        __wbg_new_with_year_month_day_hr_970fde85b640ea41: function(arg0, arg1, arg2, arg3) {
            const ret = new Date(arg0 >>> 0, arg1, arg2, arg3);
            return ret;
        },
        __wbg_new_with_year_month_day_hr_min_dc812d032417876c: function(arg0, arg1, arg2, arg3, arg4) {
            const ret = new Date(arg0 >>> 0, arg1, arg2, arg3, arg4);
            return ret;
        },
        __wbg_new_with_year_month_day_hr_min_sec_c556132f181b08c9: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            const ret = new Date(arg0 >>> 0, arg1, arg2, arg3, arg4, arg5);
            return ret;
        },
        __wbg_new_with_year_month_day_hr_min_sec_milli_b7c11a681dfe5a92: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            const ret = new Date(arg0 >>> 0, arg1, arg2, arg3, arg4, arg5, arg6);
            return ret;
        },
        __wbg_nextElementSibling_7567f8c6096dcac5: function(arg0) {
            const ret = arg0.nextElementSibling;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_nextElementSibling_e9bc9b270a9076e9: function(arg0) {
            const ret = arg0.nextElementSibling;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_nextSibling_f356004d1bf9863d: function(arg0) {
            const ret = arg0.nextSibling;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_next_9e03acdf51c4960d: function(arg0) {
            const ret = arg0.next;
            return ret;
        },
        __wbg_nodeName_f9f9e2c3d956a530: function(arg0, arg1) {
            const ret = arg1.nodeName;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_nodeType_6fe8a61baba73939: function(arg0) {
            const ret = arg0.nodeType;
            return ret;
        },
        __wbg_nodeValue_aa1e6043cebba0cd: function(arg0, arg1) {
            const ret = arg1.nodeValue;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_nonce_d863e6fe8f47aedf: function(arg0, arg1) {
            const ret = arg1.nonce;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_normalize_542eb05c1bf62ac5: function(arg0, arg1, arg2) {
            const ret = arg0.normalize(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_normalize_b0fd06b9dc556054: function(arg0) {
            arg0.normalize();
        },
        __wbg_notify_114de5bbdc397c0a: function() { return handleError(function (arg0, arg1) {
            const ret = Atomics.notify(arg0, arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_notify_b0ed980f22c7c588: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Atomics.notify(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        }, arguments); },
        __wbg_notify_be68cdefb2b808ba: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Atomics.notify(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        }, arguments); },
        __wbg_notify_bigint_3937ad40afdd28f0: function() { return handleError(function (arg0, arg1) {
            const ret = Atomics.notify_bigint(arg0, arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_now_190933fa139cc119: function() {
            const ret = Date.now();
            return ret;
        },
        __wbg_now_f565250295e2d180: function(arg0) {
            const ret = arg0.now();
            return ret;
        },
        __wbg_numbering_system_42841fdb4b090d1b: function(arg0) {
            const ret = arg0.numberingSystem;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_numeric_df7f856810ab4e10: function(arg0) {
            const ret = arg0.numeric;
            return ret;
        },
        __wbg_observe_85023e8cba523492: function(arg0, arg1) {
            arg0.observe(arg1);
        },
        __wbg_of_500f900a1fe352ff: function(arg0, arg1, arg2, arg3, arg4) {
            const ret = Array.of(arg0, arg1, arg2, arg3, arg4);
            return ret;
        },
        __wbg_of_57145fdec12d159f: function(arg0) {
            const ret = Array.of(arg0);
            return ret;
        },
        __wbg_of_5d9c1c77975668d1: function(arg0, arg1, arg2) {
            const ret = Array.of(arg0, arg1, arg2);
            return ret;
        },
        __wbg_of_7ce26f06fd728a60: function(arg0, arg1, arg2, arg3) {
            const ret = Array.of(arg0, arg1, arg2, arg3);
            return ret;
        },
        __wbg_of_f1ff4056d78bae4e: function(arg0, arg1, arg2) {
            const ret = arg0.of(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_of_f34f1ce50f299ee9: function(arg0, arg1) {
            const ret = Array.of(arg0, arg1);
            return ret;
        },
        __wbg_offsetHeight_c3c884d1082f2a03: function(arg0) {
            const ret = arg0.offsetHeight;
            return ret;
        },
        __wbg_offsetLeft_693316ff21fceb62: function(arg0) {
            const ret = arg0.offsetLeft;
            return ret;
        },
        __wbg_offsetParent_0e47ddf2dca54988: function(arg0) {
            const ret = arg0.offsetParent;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_offsetTop_a46f900932d41962: function(arg0) {
            const ret = arg0.offsetTop;
            return ret;
        },
        __wbg_offsetWidth_44642f6f44cf2fb4: function(arg0) {
            const ret = arg0.offsetWidth;
            return ret;
        },
        __wbg_offsetX_9ed7cd6abcf67d8b: function(arg0) {
            const ret = arg0.offsetX;
            return ret;
        },
        __wbg_offsetY_31fc6016c060dacd: function(arg0) {
            const ret = arg0.offsetY;
            return ret;
        },
        __wbg_ok_fb13c30bc1893039: function(arg0) {
            const ret = arg0.ok;
            return ret;
        },
        __wbg_onLine_e4578f3bdc3d560f: function(arg0) {
            const ret = arg0.onLine;
            return ret;
        },
        __wbg_onabort_490bf04e337c407f: function(arg0) {
            const ret = arg0.onabort;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onabort_567f0da37590f878: function(arg0) {
            const ret = arg0.onabort;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onabort_73e503103a709a46: function(arg0) {
            const ret = arg0.onabort;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onafterprint_20c65111e0121071: function(arg0) {
            const ret = arg0.onafterprint;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onafterscriptexecute_1db99e11d5d2f62f: function(arg0) {
            const ret = arg0.onafterscriptexecute;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onanimationcancel_29ca3b51b959568e: function(arg0) {
            const ret = arg0.onanimationcancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onanimationcancel_7bed844f5d0b2822: function(arg0) {
            const ret = arg0.onanimationcancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onanimationcancel_e229505d0f941bc5: function(arg0) {
            const ret = arg0.onanimationcancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onanimationend_0b693e3e93a39700: function(arg0) {
            const ret = arg0.onanimationend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onanimationend_87b92c589d9d9def: function(arg0) {
            const ret = arg0.onanimationend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onanimationend_966b7a2bc3d33ef9: function(arg0) {
            const ret = arg0.onanimationend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onanimationiteration_2ad2326bd65714a2: function(arg0) {
            const ret = arg0.onanimationiteration;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onanimationiteration_a5026c2ec8e17649: function(arg0) {
            const ret = arg0.onanimationiteration;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onanimationiteration_d245c47fb910de4b: function(arg0) {
            const ret = arg0.onanimationiteration;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onanimationstart_061ef47e584c96ce: function(arg0) {
            const ret = arg0.onanimationstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onanimationstart_44c2876005f65a26: function(arg0) {
            const ret = arg0.onanimationstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onanimationstart_6558fdced201f30b: function(arg0) {
            const ret = arg0.onanimationstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onappinstalled_713752e5a22dcca5: function(arg0) {
            const ret = arg0.onappinstalled;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onauxclick_a919d8cc1ee882b2: function(arg0) {
            const ret = arg0.onauxclick;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onauxclick_e036f1525dd2728d: function(arg0) {
            const ret = arg0.onauxclick;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onauxclick_e35ab00c73183a88: function(arg0) {
            const ret = arg0.onauxclick;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onbeforeinput_180ecca061c93907: function(arg0) {
            const ret = arg0.onbeforeinput;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onbeforeinput_5996e0a3c16b29b1: function(arg0) {
            const ret = arg0.onbeforeinput;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onbeforeinput_e880e751dbd464c5: function(arg0) {
            const ret = arg0.onbeforeinput;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onbeforeprint_7787bbcfe43156af: function(arg0) {
            const ret = arg0.onbeforeprint;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onbeforescriptexecute_78b03487bbd7abea: function(arg0) {
            const ret = arg0.onbeforescriptexecute;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onbeforetoggle_0aaf7f707ecdee5a: function(arg0) {
            const ret = arg0.onbeforetoggle;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onbeforetoggle_ce8342e144189ea0: function(arg0) {
            const ret = arg0.onbeforetoggle;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onbeforetoggle_d331c312a9f5c81d: function(arg0) {
            const ret = arg0.onbeforetoggle;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onbeforeunload_00cb558cc27985d2: function(arg0) {
            const ret = arg0.onbeforeunload;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onblur_32f423e2492d87f7: function(arg0) {
            const ret = arg0.onblur;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onblur_44263ec1f01572e5: function(arg0) {
            const ret = arg0.onblur;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onblur_8e64c8d06d125706: function(arg0) {
            const ret = arg0.onblur;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncancel_2462681b99f13054: function(arg0) {
            const ret = arg0.oncancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncancel_87d6ea462a42dc75: function(arg0) {
            const ret = arg0.oncancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncancel_f69709be20a573ba: function(arg0) {
            const ret = arg0.oncancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncanplay_03d6af0a2ce158c4: function(arg0) {
            const ret = arg0.oncanplay;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncanplay_3140806f4866cffb: function(arg0) {
            const ret = arg0.oncanplay;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncanplay_871bf1de19784e79: function(arg0) {
            const ret = arg0.oncanplay;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncanplaythrough_0b611aa313420905: function(arg0) {
            const ret = arg0.oncanplaythrough;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncanplaythrough_1ee4dba0113cc6bf: function(arg0) {
            const ret = arg0.oncanplaythrough;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncanplaythrough_47386632460238c1: function(arg0) {
            const ret = arg0.oncanplaythrough;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onchange_3b398602c5e1da1c: function(arg0) {
            const ret = arg0.onchange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onchange_956fda2cbb477988: function(arg0) {
            const ret = arg0.onchange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onchange_a087cd19c5194c88: function(arg0) {
            const ret = arg0.onchange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onchange_e137e7c0b1acd976: function(arg0) {
            const ret = arg0.onchange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onclick_11f72bf51c0aa910: function(arg0) {
            const ret = arg0.onclick;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onclick_276b6cc75c0443f6: function(arg0) {
            const ret = arg0.onclick;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onclick_57edd14a3f6725dc: function(arg0) {
            const ret = arg0.onclick;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onclose_131d832f4b12fee1: function(arg0) {
            const ret = arg0.onclose;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onclose_9311d00411886da5: function(arg0) {
            const ret = arg0.onclose;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onclose_9493550236fa6d01: function(arg0) {
            const ret = arg0.onclose;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onclose_fab8c178486a0dd9: function(arg0) {
            const ret = arg0.onclose;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncontextmenu_3edd5a5fdbbc0e5c: function(arg0) {
            const ret = arg0.oncontextmenu;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncontextmenu_ba343f11a3b4460c: function(arg0) {
            const ret = arg0.oncontextmenu;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncontextmenu_d2ea09d245284727: function(arg0) {
            const ret = arg0.oncontextmenu;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncopy_83cb0e56f53e787a: function(arg0) {
            const ret = arg0.oncopy;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncopy_d65b514e7bfa4067: function(arg0) {
            const ret = arg0.oncopy;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncut_5009a7e53cc77199: function(arg0) {
            const ret = arg0.oncut;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oncut_7a7a728e1f3c0bc5: function(arg0) {
            const ret = arg0.oncut;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondblclick_2c326a3d281a61ad: function(arg0) {
            const ret = arg0.ondblclick;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondblclick_ae0519a1ceb5960d: function(arg0) {
            const ret = arg0.ondblclick;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondblclick_cc3ebe9117ee9a24: function(arg0) {
            const ret = arg0.ondblclick;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondrag_7c2d44beb8f55015: function(arg0) {
            const ret = arg0.ondrag;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondrag_cc8ff5ceb21b0ee7: function(arg0) {
            const ret = arg0.ondrag;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondrag_fb79f058f374b826: function(arg0) {
            const ret = arg0.ondrag;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragend_a23ae8fd9a7dfc3a: function(arg0) {
            const ret = arg0.ondragend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragend_dca7535943d2f452: function(arg0) {
            const ret = arg0.ondragend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragend_eca286c633d92c38: function(arg0) {
            const ret = arg0.ondragend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragenter_0a593a60b8b3d460: function(arg0) {
            const ret = arg0.ondragenter;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragenter_c1f1e2c349f968c4: function(arg0) {
            const ret = arg0.ondragenter;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragenter_d562a2de5a9c3f7e: function(arg0) {
            const ret = arg0.ondragenter;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragexit_6dc92624c705fa49: function(arg0) {
            const ret = arg0.ondragexit;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragexit_a2bc8cb7e38cc7ee: function(arg0) {
            const ret = arg0.ondragexit;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragexit_b08c6d33c2590107: function(arg0) {
            const ret = arg0.ondragexit;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragleave_69aa9336cb1ec1ee: function(arg0) {
            const ret = arg0.ondragleave;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragleave_b67aa82fe3c30e68: function(arg0) {
            const ret = arg0.ondragleave;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragleave_f02bcc73d53652dc: function(arg0) {
            const ret = arg0.ondragleave;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragover_2b8d2985a5c0ee6a: function(arg0) {
            const ret = arg0.ondragover;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragover_46cafa6c89dea9b1: function(arg0) {
            const ret = arg0.ondragover;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragover_e89b9476d75feceb: function(arg0) {
            const ret = arg0.ondragover;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragstart_030348e1154a3bdc: function(arg0) {
            const ret = arg0.ondragstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragstart_2352b7a2d3aa0e05: function(arg0) {
            const ret = arg0.ondragstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondragstart_e5f4460bb653cf5c: function(arg0) {
            const ret = arg0.ondragstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondrop_585d6272fa97cc98: function(arg0) {
            const ret = arg0.ondrop;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondrop_7f1d71a235ac373c: function(arg0) {
            const ret = arg0.ondrop;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondrop_808a32c00e0b7ce8: function(arg0) {
            const ret = arg0.ondrop;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondurationchange_9cfe4767522517d1: function(arg0) {
            const ret = arg0.ondurationchange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondurationchange_c1e9602010791c85: function(arg0) {
            const ret = arg0.ondurationchange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ondurationchange_e488da3ee338a990: function(arg0) {
            const ret = arg0.ondurationchange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onemptied_0ae0e49836272ab9: function(arg0) {
            const ret = arg0.onemptied;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onemptied_0b25bbcbdf43b298: function(arg0) {
            const ret = arg0.onemptied;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onemptied_e70b4c5c3b09d980: function(arg0) {
            const ret = arg0.onemptied;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onended_1464d53ee4372283: function(arg0) {
            const ret = arg0.onended;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onended_698d11f07748ff60: function(arg0) {
            const ret = arg0.onended;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onended_f997df7784dc19e7: function(arg0) {
            const ret = arg0.onended;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onerror_3c6f4313851b21f4: function(arg0) {
            const ret = arg0.onerror;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onerror_4b13f072173b7ad1: function(arg0) {
            const ret = arg0.onerror;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onerror_5523ff047b5cc3e7: function(arg0) {
            const ret = arg0.onerror;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onerror_7884192345aadebd: function(arg0) {
            const ret = arg0.onerror;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onfocus_4144fa495aabb4f3: function(arg0) {
            const ret = arg0.onfocus;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onfocus_5a6079c418d11e73: function(arg0) {
            const ret = arg0.onfocus;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onfocus_e65492536d80ef63: function(arg0) {
            const ret = arg0.onfocus;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onfullscreenchange_1a8537fe656b0219: function(arg0) {
            const ret = arg0.onfullscreenchange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onfullscreenerror_0ccbd7ff0d64e5fa: function(arg0) {
            const ret = arg0.onfullscreenerror;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ongotpointercapture_806d50a49c3e7cf7: function(arg0) {
            const ret = arg0.ongotpointercapture;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ongotpointercapture_9b6bec34ac60f701: function(arg0) {
            const ret = arg0.ongotpointercapture;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ongotpointercapture_da73ea9b5e368063: function(arg0) {
            const ret = arg0.ongotpointercapture;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onhashchange_db41506df12a5678: function(arg0) {
            const ret = arg0.onhashchange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oninput_04a20879d75ba2d3: function(arg0) {
            const ret = arg0.oninput;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oninput_348770505b63989b: function(arg0) {
            const ret = arg0.oninput;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oninput_d0a0d852df048dab: function(arg0) {
            const ret = arg0.oninput;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oninvalid_87d70411030ead01: function(arg0) {
            const ret = arg0.oninvalid;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oninvalid_90b37df088840d60: function(arg0) {
            const ret = arg0.oninvalid;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_oninvalid_ca50df7a70a89f0b: function(arg0) {
            const ret = arg0.oninvalid;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onkeydown_410119497d960401: function(arg0) {
            const ret = arg0.onkeydown;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onkeydown_725b2b9d6ad276ba: function(arg0) {
            const ret = arg0.onkeydown;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onkeydown_af96c62b53827242: function(arg0) {
            const ret = arg0.onkeydown;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onkeypress_720852185017a041: function(arg0) {
            const ret = arg0.onkeypress;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onkeypress_839bb27ebcffc5cf: function(arg0) {
            const ret = arg0.onkeypress;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onkeypress_8c4e6ea59273cc1b: function(arg0) {
            const ret = arg0.onkeypress;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onkeyup_2e28455b6a234c31: function(arg0) {
            const ret = arg0.onkeyup;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onkeyup_98879d6aa03970ee: function(arg0) {
            const ret = arg0.onkeyup;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onkeyup_f7d38dc687a5a505: function(arg0) {
            const ret = arg0.onkeyup;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onlanguagechange_502aa73526f878bf: function(arg0) {
            const ret = arg0.onlanguagechange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onload_56a0ae934d9725a2: function(arg0) {
            const ret = arg0.onload;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onload_9bb26bb61248a7d3: function(arg0) {
            const ret = arg0.onload;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onload_c4c9d49f4e8f4dd2: function(arg0) {
            const ret = arg0.onload;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onloadeddata_345444d58dcb1ec8: function(arg0) {
            const ret = arg0.onloadeddata;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onloadeddata_68849886df77512c: function(arg0) {
            const ret = arg0.onloadeddata;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onloadeddata_d37a26a779949286: function(arg0) {
            const ret = arg0.onloadeddata;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onloadedmetadata_3cfa1e151bffff58: function(arg0) {
            const ret = arg0.onloadedmetadata;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onloadedmetadata_615ca34b78dd40fd: function(arg0) {
            const ret = arg0.onloadedmetadata;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onloadedmetadata_ee9bd2c4044ed62f: function(arg0) {
            const ret = arg0.onloadedmetadata;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onloadend_0a7eb72364f8d7f6: function(arg0) {
            const ret = arg0.onloadend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onloadend_36ff4ea9291af34f: function(arg0) {
            const ret = arg0.onloadend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onloadend_574253c6507874bd: function(arg0) {
            const ret = arg0.onloadend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onloadstart_432f0d1c6d909400: function(arg0) {
            const ret = arg0.onloadstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onloadstart_53797ce2b3d109d4: function(arg0) {
            const ret = arg0.onloadstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onloadstart_8a2f45fcce1e2774: function(arg0) {
            const ret = arg0.onloadstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onlostpointercapture_ae72d51db82f9fdf: function(arg0) {
            const ret = arg0.onlostpointercapture;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onlostpointercapture_ce8a77fa4f2f2248: function(arg0) {
            const ret = arg0.onlostpointercapture;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onlostpointercapture_d9201c09078bb38d: function(arg0) {
            const ret = arg0.onlostpointercapture;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmessage_47f4fa9de5b64bb3: function(arg0) {
            const ret = arg0.onmessage;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmessage_6e1a7226dfbc7178: function(arg0) {
            const ret = arg0.onmessage;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmessageerror_aa0b6cdacd1a5991: function(arg0) {
            const ret = arg0.onmessageerror;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmousedown_47a1b5aee752435c: function(arg0) {
            const ret = arg0.onmousedown;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmousedown_ae9800ca3140ba93: function(arg0) {
            const ret = arg0.onmousedown;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmousedown_c76081a3480634da: function(arg0) {
            const ret = arg0.onmousedown;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseenter_2a6b107eae1794ff: function(arg0) {
            const ret = arg0.onmouseenter;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseenter_74b39509d0c74f7e: function(arg0) {
            const ret = arg0.onmouseenter;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseenter_c4c2b33df9e87452: function(arg0) {
            const ret = arg0.onmouseenter;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseleave_444ef325fa431d5b: function(arg0) {
            const ret = arg0.onmouseleave;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseleave_68c3ad6219dfff91: function(arg0) {
            const ret = arg0.onmouseleave;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseleave_eab9f5cf2d595c15: function(arg0) {
            const ret = arg0.onmouseleave;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmousemove_70b990e47e358ea1: function(arg0) {
            const ret = arg0.onmousemove;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmousemove_8b9fa5b4602c25d0: function(arg0) {
            const ret = arg0.onmousemove;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmousemove_d719ffdb370300c4: function(arg0) {
            const ret = arg0.onmousemove;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseout_70554dacfc9e7969: function(arg0) {
            const ret = arg0.onmouseout;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseout_c4f3d24a1bb46c1c: function(arg0) {
            const ret = arg0.onmouseout;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseout_ea3d8ee6dec32fe4: function(arg0) {
            const ret = arg0.onmouseout;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseover_57b7ae63cf7de4c6: function(arg0) {
            const ret = arg0.onmouseover;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseover_a89b5e99faa2078d: function(arg0) {
            const ret = arg0.onmouseover;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseover_ad6e805916a83f85: function(arg0) {
            const ret = arg0.onmouseover;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseup_073b72b8443c2a0e: function(arg0) {
            const ret = arg0.onmouseup;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseup_33f3a9d0d373dc3a: function(arg0) {
            const ret = arg0.onmouseup;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onmouseup_dcf72ae741e466b7: function(arg0) {
            const ret = arg0.onmouseup;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onoffline_f712701ca00d516c: function(arg0) {
            const ret = arg0.onoffline;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ononline_b20d04a134d05279: function(arg0) {
            const ret = arg0.ononline;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onopen_e67c8e5baa65a183: function(arg0) {
            const ret = arg0.onopen;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onorientationchange_fdaf7018c5fb28f5: function(arg0) {
            const ret = arg0.onorientationchange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpagehide_9c819611b22eb3ac: function(arg0) {
            const ret = arg0.onpagehide;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpageshow_69050e626e7c796c: function(arg0) {
            const ret = arg0.onpageshow;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpaste_a035ae5ad43f21ff: function(arg0) {
            const ret = arg0.onpaste;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpaste_f24c603e9fe16744: function(arg0) {
            const ret = arg0.onpaste;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpause_600dd87c13a13c7b: function(arg0) {
            const ret = arg0.onpause;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpause_720d40d7af3a9ca3: function(arg0) {
            const ret = arg0.onpause;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpause_ab3eede41ce15181: function(arg0) {
            const ret = arg0.onpause;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onplay_b974a458e16e17f1: function(arg0) {
            const ret = arg0.onplay;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onplay_c91ddf49aa1f7254: function(arg0) {
            const ret = arg0.onplay;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onplay_d328c3b92d0f0f95: function(arg0) {
            const ret = arg0.onplay;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onplaying_17caab4c901cc93f: function(arg0) {
            const ret = arg0.onplaying;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onplaying_2d2510373caa9637: function(arg0) {
            const ret = arg0.onplaying;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onplaying_88a3b2e2e5a6d172: function(arg0) {
            const ret = arg0.onplaying;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointercancel_927b54cdce5a1733: function(arg0) {
            const ret = arg0.onpointercancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointercancel_b10305b4668b7ac9: function(arg0) {
            const ret = arg0.onpointercancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointercancel_ec7f18a12b46929a: function(arg0) {
            const ret = arg0.onpointercancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerdown_45afa982fe0a5767: function(arg0) {
            const ret = arg0.onpointerdown;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerdown_48d7df9ad0f28c54: function(arg0) {
            const ret = arg0.onpointerdown;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerdown_ab1202b889ef7788: function(arg0) {
            const ret = arg0.onpointerdown;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerenter_3918f71afc0b3660: function(arg0) {
            const ret = arg0.onpointerenter;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerenter_90da898e267722bf: function(arg0) {
            const ret = arg0.onpointerenter;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerenter_e4878017476d6d16: function(arg0) {
            const ret = arg0.onpointerenter;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerleave_87f1dba21d6f2297: function(arg0) {
            const ret = arg0.onpointerleave;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerleave_b368ed075b670131: function(arg0) {
            const ret = arg0.onpointerleave;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerleave_d271ba85a66ea1e4: function(arg0) {
            const ret = arg0.onpointerleave;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerlockchange_8159dac1de04c92c: function(arg0) {
            const ret = arg0.onpointerlockchange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerlockerror_e3cfaa648d09c9b5: function(arg0) {
            const ret = arg0.onpointerlockerror;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointermove_1c411665535c5fee: function(arg0) {
            const ret = arg0.onpointermove;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointermove_57c8412fbe089d6a: function(arg0) {
            const ret = arg0.onpointermove;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointermove_cf1d3a8c1d39b787: function(arg0) {
            const ret = arg0.onpointermove;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerout_6f3434274e7f0bb3: function(arg0) {
            const ret = arg0.onpointerout;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerout_ab844f3d7cbe0e60: function(arg0) {
            const ret = arg0.onpointerout;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerout_e404ee77f905949e: function(arg0) {
            const ret = arg0.onpointerout;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerover_0d5715fbb214c609: function(arg0) {
            const ret = arg0.onpointerover;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerover_e149663329ac863a: function(arg0) {
            const ret = arg0.onpointerover;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerover_e34191a0df8de57f: function(arg0) {
            const ret = arg0.onpointerover;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerup_2e2d2a5b733e205b: function(arg0) {
            const ret = arg0.onpointerup;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerup_3c8fbc358f593b5d: function(arg0) {
            const ret = arg0.onpointerup;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpointerup_cdf064efe81a51c0: function(arg0) {
            const ret = arg0.onpointerup;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onpopstate_26d1fff1fe895664: function(arg0) {
            const ret = arg0.onpopstate;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onprogress_4d80444b4abffe5b: function(arg0) {
            const ret = arg0.onprogress;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onprogress_54b2d2ea1072eda3: function(arg0) {
            const ret = arg0.onprogress;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onprogress_5e0cbe1da709df37: function(arg0) {
            const ret = arg0.onprogress;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onratechange_a16f036b0fd61b9e: function(arg0) {
            const ret = arg0.onratechange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onratechange_d52068ec9803b09e: function(arg0) {
            const ret = arg0.onratechange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onratechange_f5e45887f3f1bbf4: function(arg0) {
            const ret = arg0.onratechange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onreadystatechange_dfab1646c64fd2b2: function(arg0) {
            const ret = arg0.onreadystatechange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onreset_550e6868766be95f: function(arg0) {
            const ret = arg0.onreset;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onreset_8103e465dcfeed4d: function(arg0) {
            const ret = arg0.onreset;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onreset_f71968a48461b95b: function(arg0) {
            const ret = arg0.onreset;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onresize_02fb8096935b5f8d: function(arg0) {
            const ret = arg0.onresize;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onresize_95437cc190b7dbe1: function(arg0) {
            const ret = arg0.onresize;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onresize_bcbfa0ed4678bc5b: function(arg0) {
            const ret = arg0.onresize;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onresourcetimingbufferfull_e23ea3b1ae96bce7: function(arg0) {
            const ret = arg0.onresourcetimingbufferfull;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onscroll_0bc36afaec1c0165: function(arg0) {
            const ret = arg0.onscroll;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onscroll_2093dd92cf9b9d9f: function(arg0) {
            const ret = arg0.onscroll;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onscroll_34be50178fbf6553: function(arg0) {
            const ret = arg0.onscroll;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onseeked_03c4cb801d9d3578: function(arg0) {
            const ret = arg0.onseeked;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onseeked_a55613a9e47d2bdd: function(arg0) {
            const ret = arg0.onseeked;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onseeked_c22177c8783e8232: function(arg0) {
            const ret = arg0.onseeked;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onseeking_92ca50be4278c40a: function(arg0) {
            const ret = arg0.onseeking;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onseeking_a02cddb7e655a367: function(arg0) {
            const ret = arg0.onseeking;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onseeking_f0a41c307f17ba54: function(arg0) {
            const ret = arg0.onseeking;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onselect_a51f7a0e598d16f4: function(arg0) {
            const ret = arg0.onselect;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onselect_b43de057f306f68e: function(arg0) {
            const ret = arg0.onselect;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onselect_b8aaa2573be7635f: function(arg0) {
            const ret = arg0.onselect;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onselectionchange_8c5fcb554c592946: function(arg0) {
            const ret = arg0.onselectionchange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onselectstart_b5a76d44e971f6b6: function(arg0) {
            const ret = arg0.onselectstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onselectstart_b86adf2c661360eb: function(arg0) {
            const ret = arg0.onselectstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onselectstart_ca3c7496790a7223: function(arg0) {
            const ret = arg0.onselectstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onshow_045832b0e7d620b5: function(arg0) {
            const ret = arg0.onshow;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onshow_543c6bcbd9952bfc: function(arg0) {
            const ret = arg0.onshow;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onshow_8783c3d46aeb42c7: function(arg0) {
            const ret = arg0.onshow;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onstalled_0aff0bbe8fc949d4: function(arg0) {
            const ret = arg0.onstalled;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onstalled_40c936d94618ea29: function(arg0) {
            const ret = arg0.onstalled;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onstalled_acf02e7d24d5f948: function(arg0) {
            const ret = arg0.onstalled;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onstorage_54d5dc966a6fe937: function(arg0) {
            const ret = arg0.onstorage;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onsubmit_2747a83c3dd21864: function(arg0) {
            const ret = arg0.onsubmit;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onsubmit_4cdc7ec733b77845: function(arg0) {
            const ret = arg0.onsubmit;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onsubmit_b64b960e754a64e9: function(arg0) {
            const ret = arg0.onsubmit;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onsuspend_26286058abf96364: function(arg0) {
            const ret = arg0.onsuspend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onsuspend_8dcdd2d3149e47a3: function(arg0) {
            const ret = arg0.onsuspend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onsuspend_94c7ee3fb183d17b: function(arg0) {
            const ret = arg0.onsuspend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontimeupdate_1317d609be526b85: function(arg0) {
            const ret = arg0.ontimeupdate;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontimeupdate_8bbb33bb755e8626: function(arg0) {
            const ret = arg0.ontimeupdate;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontimeupdate_fe9c1d7137dbee8a: function(arg0) {
            const ret = arg0.ontimeupdate;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontoggle_ac0ca83cccd2d377: function(arg0) {
            const ret = arg0.ontoggle;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontoggle_c3f35c53de284af4: function(arg0) {
            const ret = arg0.ontoggle;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontoggle_d6a14cd549ad8844: function(arg0) {
            const ret = arg0.ontoggle;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontouchcancel_2966a09a15d5ed77: function(arg0) {
            const ret = arg0.ontouchcancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontouchcancel_aba8b9f1ad253abf: function(arg0) {
            const ret = arg0.ontouchcancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontouchcancel_fbfad14a00b44224: function(arg0) {
            const ret = arg0.ontouchcancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontouchend_4f0ddd7ad2cd5749: function(arg0) {
            const ret = arg0.ontouchend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontouchend_58cb6c3b24425b77: function(arg0) {
            const ret = arg0.ontouchend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontouchend_d98b59f832738e50: function(arg0) {
            const ret = arg0.ontouchend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontouchmove_3940b8f282bf7c64: function(arg0) {
            const ret = arg0.ontouchmove;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontouchmove_7f81354790cfbb45: function(arg0) {
            const ret = arg0.ontouchmove;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontouchmove_dbc85ed148282f83: function(arg0) {
            const ret = arg0.ontouchmove;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontouchstart_6db7afd59f49de72: function(arg0) {
            const ret = arg0.ontouchstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontouchstart_8f67ef0ad242e3c0: function(arg0) {
            const ret = arg0.ontouchstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontouchstart_a0c528a0a800e12d: function(arg0) {
            const ret = arg0.ontouchstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontransitioncancel_12f1661256967353: function(arg0) {
            const ret = arg0.ontransitioncancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontransitioncancel_3e704271e85cfa04: function(arg0) {
            const ret = arg0.ontransitioncancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontransitioncancel_e0d20d85f11d0b0e: function(arg0) {
            const ret = arg0.ontransitioncancel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontransitionend_698c378aab3e316e: function(arg0) {
            const ret = arg0.ontransitionend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontransitionend_cc785e54ccd40f41: function(arg0) {
            const ret = arg0.ontransitionend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontransitionend_dd5ccac2d3e0a6d6: function(arg0) {
            const ret = arg0.ontransitionend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontransitionrun_2ff42a27a7473511: function(arg0) {
            const ret = arg0.ontransitionrun;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontransitionrun_86124cfeea90ca3f: function(arg0) {
            const ret = arg0.ontransitionrun;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontransitionrun_a9168aec32d0e0b6: function(arg0) {
            const ret = arg0.ontransitionrun;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontransitionstart_6bd68c8f2aadafb3: function(arg0) {
            const ret = arg0.ontransitionstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontransitionstart_de76b2f917ac730c: function(arg0) {
            const ret = arg0.ontransitionstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ontransitionstart_e3217bd4019bec93: function(arg0) {
            const ret = arg0.ontransitionstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onunload_555bfd2642d5fb68: function(arg0) {
            const ret = arg0.onunload;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onvisibilitychange_b7938805d0854557: function(arg0) {
            const ret = arg0.onvisibilitychange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onvolumechange_303712799c7b43bf: function(arg0) {
            const ret = arg0.onvolumechange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onvolumechange_6fcea02c54ca5703: function(arg0) {
            const ret = arg0.onvolumechange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onvolumechange_a99578c938378483: function(arg0) {
            const ret = arg0.onvolumechange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onvrdisplayactivate_708e79b32505cb85: function(arg0) {
            const ret = arg0.onvrdisplayactivate;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onvrdisplayconnect_c0f02e82612d47e8: function(arg0) {
            const ret = arg0.onvrdisplayconnect;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onvrdisplaydeactivate_201849bf5f495673: function(arg0) {
            const ret = arg0.onvrdisplaydeactivate;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onvrdisplaydisconnect_d41ba2d4d393ab3f: function(arg0) {
            const ret = arg0.onvrdisplaydisconnect;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onvrdisplaypresentchange_68ba690a1cb3640b: function(arg0) {
            const ret = arg0.onvrdisplaypresentchange;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwaiting_6309eada31965618: function(arg0) {
            const ret = arg0.onwaiting;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwaiting_81ee375957576d69: function(arg0) {
            const ret = arg0.onwaiting;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwaiting_823c938489d8753a: function(arg0) {
            const ret = arg0.onwaiting;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwebkitanimationend_0e952cfe8333ad6e: function(arg0) {
            const ret = arg0.onwebkitanimationend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwebkitanimationend_70db3cf5b37e0342: function(arg0) {
            const ret = arg0.onwebkitanimationend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwebkitanimationend_f39f9bfa6e56206c: function(arg0) {
            const ret = arg0.onwebkitanimationend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwebkitanimationiteration_30e7a5e3d45bab2d: function(arg0) {
            const ret = arg0.onwebkitanimationiteration;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwebkitanimationiteration_710fccc8123a5d19: function(arg0) {
            const ret = arg0.onwebkitanimationiteration;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwebkitanimationiteration_d29aaf33aa515ad1: function(arg0) {
            const ret = arg0.onwebkitanimationiteration;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwebkitanimationstart_66648089e177a892: function(arg0) {
            const ret = arg0.onwebkitanimationstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwebkitanimationstart_d9469a0c513bc826: function(arg0) {
            const ret = arg0.onwebkitanimationstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwebkitanimationstart_f6e830ec3ce47e9a: function(arg0) {
            const ret = arg0.onwebkitanimationstart;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwebkittransitionend_124c9d90da0947e3: function(arg0) {
            const ret = arg0.onwebkittransitionend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwebkittransitionend_213172219c70ae39: function(arg0) {
            const ret = arg0.onwebkittransitionend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwebkittransitionend_b39f76abd22e7b50: function(arg0) {
            const ret = arg0.onwebkittransitionend;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwheel_5a326616439d2057: function(arg0) {
            const ret = arg0.onwheel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwheel_6ec228b9fb784a60: function(arg0) {
            const ret = arg0.onwheel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_onwheel_dd445513b3db2ea3: function(arg0) {
            const ret = arg0.onwheel;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_open_15bff00d3e10d605: function() { return handleError(function (arg0) {
            const ret = arg0.open();
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_open_177ad1dcc06b9106: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            const ret = arg0.open(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_open_de4678bca285422e: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.open(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_open_feb75fe4d9971c50: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.open(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_opener_ffb66d56735ebc3a: function() { return handleError(function (arg0) {
            const ret = arg0.opener;
            return ret;
        }, arguments); },
        __wbg_orientation_58510a33769d9817: function(arg0) {
            const ret = arg0.orientation;
            return ret;
        },
        __wbg_origin_40f2aaf3874838f6: function(arg0, arg1) {
            const ret = arg1.origin;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_origin_a66aff536126dd36: function(arg0, arg1) {
            const ret = arg1.origin;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_origin_c2b4f0fd247d148e: function(arg0, arg1) {
            const ret = arg1.origin;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_origin_d4fcb992a8589439: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.origin;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_origin_f03cc1c01dde2956: function(arg0, arg1) {
            const ret = arg1.origin;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_outerHTML_d5c76d4f696f9df8: function(arg0, arg1) {
            const ret = arg1.outerHTML;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_outerHeight_e3acf6ce8152a048: function() { return handleError(function (arg0) {
            const ret = arg0.outerHeight;
            return ret;
        }, arguments); },
        __wbg_outerWidth_49b50428ae1af39d: function() { return handleError(function (arg0) {
            const ret = arg0.outerWidth;
            return ret;
        }, arguments); },
        __wbg_ownKeys_0587b8fe286a40e6: function() { return handleError(function (arg0) {
            const ret = Reflect.ownKeys(arg0);
            return ret;
        }, arguments); },
        __wbg_ownerDocument_3382cef744616ba1: function(arg0) {
            const ret = arg0.ownerDocument;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ownerNode_714aa47ba2d9808d: function(arg0) {
            const ret = arg0.ownerNode;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ownerRule_63fc60ef96a82a9e: function(arg0) {
            const ret = arg0.ownerRule;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_padEnd_04813523b74006ff: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.padEnd(arg1 >>> 0, getStringFromWasm0(arg2, arg3));
            return ret;
        },
        __wbg_padStart_5edce3fd8ef09a71: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.padStart(arg1 >>> 0, getStringFromWasm0(arg2, arg3));
            return ret;
        },
        __wbg_pageXOffset_fb7be929d7e612ad: function() { return handleError(function (arg0) {
            const ret = arg0.pageXOffset;
            return ret;
        }, arguments); },
        __wbg_pageX_ce8fde350462ec83: function(arg0) {
            const ret = arg0.pageX;
            return ret;
        },
        __wbg_pageX_e6c22dee653091ec: function(arg0) {
            const ret = arg0.pageX;
            return ret;
        },
        __wbg_pageYOffset_c864be80df4f3b85: function() { return handleError(function (arg0) {
            const ret = arg0.pageYOffset;
            return ret;
        }, arguments); },
        __wbg_pageY_d267a50f1054ace5: function(arg0) {
            const ret = arg0.pageY;
            return ret;
        },
        __wbg_pageY_d74f2a3362593399: function(arg0) {
            const ret = arg0.pageY;
            return ret;
        },
        __wbg_parentElement_8b5d8fe9f2319ca8: function(arg0) {
            const ret = arg0.parentElement;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_parentNode_6ee3190f9ba96b9b: function(arg0) {
            const ret = arg0.parentNode;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_parentRule_60677338fdc68156: function(arg0) {
            const ret = arg0.parentRule;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_parentRule_8e5ae99bbb4265e1: function(arg0) {
            const ret = arg0.parentRule;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_parentStyleSheet_042ccecada61825a: function(arg0) {
            const ret = arg0.parentStyleSheet;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_parentStyleSheet_7b72bfe50be7b845: function(arg0) {
            const ret = arg0.parentStyleSheet;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_parent_71a796be34747d53: function() { return handleError(function (arg0) {
            const ret = arg0.parent;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_parseFloat_b4e60d3271be6006: function(arg0, arg1) {
            const ret = Number.parseFloat(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_parseFloat_daa3e29bcf9b82dd: function(arg0, arg1) {
            const ret = parseFloat(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_parseInt_4654c4783fed600a: function(arg0, arg1, arg2) {
            const ret = parseInt(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_parseInt_7363785b82d7e6d8: function(arg0, arg1, arg2) {
            const ret = Number.parseInt(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        },
        __wbg_parse_03863847d06c4e89: function() { return handleError(function (arg0, arg1) {
            const ret = JSON.parse(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_parse_a7c8a8cd2f5065db: function(arg0, arg1) {
            const ret = Date.parse(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_password_24864dd506af956a: function(arg0, arg1) {
            const ret = arg1.password;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_password_40b34fee544d7bd6: function(arg0, arg1) {
            const ret = arg1.password;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_pathname_32003e652dc11308: function(arg0, arg1) {
            const ret = arg1.pathname;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_pathname_626d5a44ea16c0c9: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.pathname;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_pathname_970b828999d759fb: function(arg0, arg1) {
            const ret = arg1.pathname;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_pattern_d6b02424d9ca2064: function(arg0, arg1) {
            const ret = arg1.pattern;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_pause_4138c10c5461e47b: function() {
            Atomics.pause();
        },
        __wbg_pause_with_hint_c533a62ae77f5c50: function(arg0) {
            Atomics.pause_with_hint(arg0 >>> 0);
        },
        __wbg_performance_68499ca0718837f5: function(arg0) {
            const ret = arg0.performance;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_ping_ba67132bd9a0eb2d: function(arg0, arg1) {
            const ret = arg1.ping;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_placeholder_8330cb7d122a38d3: function(arg0, arg1) {
            const ret = arg1.placeholder;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_placeholder_89c1aa68d059d202: function(arg0, arg1) {
            const ret = arg1.placeholder;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_platform_f49078a3b1bd6ee3: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.platform;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_pointerId_be45f553bf33ecdd: function(arg0) {
            const ret = arg0.pointerId;
            return ret;
        },
        __wbg_pointerLockElement_05a2e24dadadb9eb: function(arg0) {
            const ret = arg0.pointerLockElement;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_pointerType_2da062eba6f661c0: function(arg0, arg1) {
            const ret = arg1.pointerType;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_popoverTargetAction_d800905775199d16: function(arg0, arg1) {
            const ret = arg1.popoverTargetAction;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_popoverTargetElement_a9284e5af438eb26: function(arg0) {
            const ret = arg0.popoverTargetElement;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_popover_56277113d99ea620: function(arg0, arg1) {
            const ret = arg1.popover;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_port_192ee8b5d17b403a: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.port;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_port_7eb5f758c0de4050: function(arg0, arg1) {
            const ret = arg1.port;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_port_e85d26e70dff440a: function(arg0, arg1) {
            const ret = arg1.port;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_ports_91ab97df99186bdf: function(arg0) {
            const ret = arg0.ports;
            return ret;
        },
        __wbg_postMessage_6b98a3535abeb431: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.postMessage(arg1, getStringFromWasm0(arg2, arg3));
        }, arguments); },
        __wbg_postMessage_7e572c8f7f6fef5c: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.postMessage(arg1, getStringFromWasm0(arg2, arg3), arg4);
        }, arguments); },
        __wbg_pow_e5e9dc49b4dfbe48: function(arg0, arg1) {
            const ret = Math.pow(arg0, arg1);
            return ret;
        },
        __wbg_preferredStyleSheetSet_42535666c0e06855: function(arg0, arg1) {
            const ret = arg1.preferredStyleSheetSet;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_prefix_ba35ab276f7dcfa1: function(arg0, arg1) {
            const ret = arg1.prefix;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_prepend_00d3335a90ec2add: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_prepend_021c710c82222192: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.prepend(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        }, arguments); },
        __wbg_prepend_083d4d88306f5246: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.prepend(arg1, arg2);
        }, arguments); },
        __wbg_prepend_0e70df79cf845276: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        }, arguments); },
        __wbg_prepend_0f81ad095305cffc: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        }, arguments); },
        __wbg_prepend_10217b1c269bdc8a: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        }, arguments); },
        __wbg_prepend_1e669ee057336db3: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_prepend_212796e1cc72efdf: function() { return handleError(function (arg0) {
            arg0.prepend();
        }, arguments); },
        __wbg_prepend_21459d663b3bb2c0: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.prepend(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        }, arguments); },
        __wbg_prepend_2558faad1849b7a2: function() { return handleError(function (arg0) {
            arg0.prepend();
        }, arguments); },
        __wbg_prepend_294ff208960a5d8e: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.prepend(arg1, arg2, arg3, arg4, arg5, arg6);
        }, arguments); },
        __wbg_prepend_2cdc7181960480e8: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.prepend(arg1, arg2, arg3, arg4, arg5, arg6);
        }, arguments); },
        __wbg_prepend_3112927787977076: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_prepend_38b27607c86ac172: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        }, arguments); },
        __wbg_prepend_44099f3e21bbdb7b: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.prepend(arg1, arg2, arg3, arg4);
        }, arguments); },
        __wbg_prepend_44ba172bcc62a367: function() { return handleError(function (arg0, arg1) {
            arg0.prepend(arg1);
        }, arguments); },
        __wbg_prepend_5206c1fc176c8431: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.prepend(arg1, arg2, arg3);
        }, arguments); },
        __wbg_prepend_55fdb746d02cb8c5: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.prepend(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_prepend_5b49021cb3a34b3d: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.prepend(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_prepend_5fb01eec847027a9: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        }, arguments); },
        __wbg_prepend_6c812afa7e9a6869: function() { return handleError(function (arg0, arg1) {
            arg0.prepend(...arg1);
        }, arguments); },
        __wbg_prepend_6e4555001e8f8df0: function() { return handleError(function (arg0, arg1) {
            arg0.prepend(...arg1);
        }, arguments); },
        __wbg_prepend_70df86edf0b405f7: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        }, arguments); },
        __wbg_prepend_762484980c371df0: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.prepend(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_prepend_771336da69bcc317: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.prepend(arg1, arg2, arg3, arg4);
        }, arguments); },
        __wbg_prepend_774fdf05cc1109e6: function() { return handleError(function (arg0) {
            arg0.prepend();
        }, arguments); },
        __wbg_prepend_79f77fa480873368: function() { return handleError(function (arg0, arg1) {
            arg0.prepend(...arg1);
        }, arguments); },
        __wbg_prepend_7b87cffd74667309: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        }, arguments); },
        __wbg_prepend_807ab610f7acfc3e: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        }, arguments); },
        __wbg_prepend_83769f20753301d6: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.prepend(arg1, arg2, arg3);
        }, arguments); },
        __wbg_prepend_964d88ac3fcf8ee8: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        }, arguments); },
        __wbg_prepend_9a18a5ed60682857: function() { return handleError(function (arg0, arg1) {
            arg0.prepend(arg1);
        }, arguments); },
        __wbg_prepend_a7cf97fa511bba37: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.prepend(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_prepend_b3d54d13630e364f: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        }, arguments); },
        __wbg_prepend_bfcc475afb9ce455: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_prepend_c1a5a43bc8888b7e: function() { return handleError(function (arg0, arg1) {
            arg0.prepend(...arg1);
        }, arguments); },
        __wbg_prepend_c49f0f8500ec95ea: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.prepend(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_prepend_c7948146a310a12e: function() { return handleError(function (arg0) {
            arg0.prepend();
        }, arguments); },
        __wbg_prepend_cb39dae6ddeebe49: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.prepend(arg1, arg2);
        }, arguments); },
        __wbg_prepend_cfb8e865e153ca36: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.prepend(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        }, arguments); },
        __wbg_prepend_d3002cffa423a22b: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.prepend(arg1, arg2);
        }, arguments); },
        __wbg_prepend_d4f848c9771e40e9: function() { return handleError(function (arg0) {
            arg0.prepend();
        }, arguments); },
        __wbg_prepend_d65a48025f317527: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.prepend(arg1, arg2, arg3, arg4);
        }, arguments); },
        __wbg_prepend_d737c0c11066af9d: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_prepend_dcac584094e51af3: function() { return handleError(function (arg0, arg1) {
            arg0.prepend(...arg1);
        }, arguments); },
        __wbg_prepend_dff6b514fa531839: function() { return handleError(function (arg0) {
            arg0.prepend();
        }, arguments); },
        __wbg_prepend_e238167891db5b61: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_prepend_ee77f76b8d4b2fd7: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.prepend(arg1, arg2, arg3);
        }, arguments); },
        __wbg_prepend_ef0a7e3009f7b52c: function() { return handleError(function (arg0, arg1) {
            arg0.prepend(arg1);
        }, arguments); },
        __wbg_prepend_f3afe436697797ae: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        }, arguments); },
        __wbg_prepend_f3c186de919783d4: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.prepend(arg1, arg2, arg3, arg4, arg5, arg6);
        }, arguments); },
        __wbg_prepend_f488014125d3d10b: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.prepend(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_prepend_f77ea686f8199cbd: function() { return handleError(function (arg0, arg1) {
            arg0.prepend(...arg1);
        }, arguments); },
        __wbg_prepend_fe2216aad4f4a8b6: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.prepend(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        }, arguments); },
        __wbg_pressure_e31046ec6e6263d3: function(arg0) {
            const ret = arg0.pressure;
            return ret;
        },
        __wbg_preventDefault_4902f41a1b31bedd: function(arg0) {
            arg0.preventDefault();
        },
        __wbg_previousElementSibling_8a594d0f88507cb9: function(arg0) {
            const ret = arg0.previousElementSibling;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_previousElementSibling_8bc23ba7e2be6d78: function(arg0) {
            const ret = arg0.previousElementSibling;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_previousSibling_5bd355d0e4381b16: function(arg0) {
            const ret = arg0.previousSibling;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_print_85ec812020dd7ccf: function() { return handleError(function (arg0) {
            arg0.print();
        }, arguments); },
        __wbg_product_a69274ddda9cef4a: function(arg0, arg1) {
            const ret = arg1.product;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_profileEnd_0e8718487dab1dc3: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.profileEnd(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_profileEnd_4c7e483cb834b6e5: function(arg0, arg1) {
            console.profileEnd(arg0, arg1);
        },
        __wbg_profileEnd_55434cedc2769e97: function(arg0) {
            console.profileEnd(...arg0);
        },
        __wbg_profileEnd_6adb2e95f212b5e5: function(arg0, arg1, arg2, arg3, arg4) {
            console.profileEnd(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_profileEnd_91f0a7894c613727: function(arg0, arg1, arg2, arg3) {
            console.profileEnd(arg0, arg1, arg2, arg3);
        },
        __wbg_profileEnd_9f5fda50d2184a59: function(arg0) {
            console.profileEnd(arg0);
        },
        __wbg_profileEnd_cdb94e3e1c78f03f: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.profileEnd(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_profileEnd_eedd185fd185aa8f: function(arg0, arg1, arg2) {
            console.profileEnd(arg0, arg1, arg2);
        },
        __wbg_profileEnd_efc11d5f7a1f2dc0: function() {
            console.profileEnd();
        },
        __wbg_profile_0be0edcb57abb4f5: function() {
            console.profile();
        },
        __wbg_profile_0f8d299a639ccdac: function(arg0) {
            console.profile(...arg0);
        },
        __wbg_profile_1d598debd35b17bc: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.profile(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_profile_861b30b629c615ab: function(arg0, arg1, arg2, arg3) {
            console.profile(arg0, arg1, arg2, arg3);
        },
        __wbg_profile_8faaa62f9ef5e248: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.profile(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_profile_981711ef4a22941b: function(arg0, arg1, arg2, arg3, arg4) {
            console.profile(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_profile_a52313f118ec781b: function(arg0, arg1, arg2) {
            console.profile(arg0, arg1, arg2);
        },
        __wbg_profile_a6965505643f6597: function(arg0) {
            console.profile(arg0);
        },
        __wbg_profile_e7c533bc2d7f93ea: function(arg0, arg1) {
            console.profile(arg0, arg1);
        },
        __wbg_prompt_37ff687ea61493ab: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg1.prompt(getStringFromWasm0(arg2, arg3));
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_prompt_43b42d04aa3a4b95: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.prompt();
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_prompt_79e380cdd9b43d9e: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            const ret = arg1.prompt(getStringFromWasm0(arg2, arg3), getStringFromWasm0(arg4, arg5));
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_protocol_337b1d784a85a170: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.protocol;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_protocol_366144f05ff588b1: function(arg0, arg1) {
            const ret = arg1.protocol;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_protocol_563b94a4dcfb4d0d: function(arg0, arg1) {
            const ret = arg1.protocol;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_protocol_6b00262d9dfb319d: function(arg0, arg1) {
            const ret = arg1.protocol;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_prototypesetcall_3249fc62a0fafa30: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_3560ffd2e7034ba8: function(arg0, arg1, arg2) {
            Int32Array.prototype.set.call(getArrayI32FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_458b35f147b77a90: function(arg0, arg1, arg2) {
            Int8Array.prototype.set.call(getArrayI8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_50d11ab447a6ea86: function(arg0, arg1, arg2) {
            Uint16Array.prototype.set.call(getArrayU16FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_59640349d2c6d881: function(arg0, arg1, arg2) {
            Uint32Array.prototype.set.call(getArrayU32FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_5f7ae2ad4c30d834: function(arg0, arg1, arg2) {
            Uint8ClampedArray.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_6239d0967941c8d9: function(arg0, arg1, arg2) {
            Float32Array.prototype.set.call(getArrayF32FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_7bffe90c634609ef: function(arg0, arg1, arg2) {
            Int16Array.prototype.set.call(getArrayI16FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_93008fb6e7568902: function(arg0, arg1, arg2) {
            BigInt64Array.prototype.set.call(getArrayI64FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_ceea13668b8fe40b: function(arg0, arg1, arg2) {
            BigUint64Array.prototype.set.call(getArrayU64FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_d1ae8885a2e9d458: function(arg0, arg1, arg2) {
            Float64Array.prototype.set.call(getArrayF64FromWasm0(arg0, arg1), arg2);
        },
        __wbg_pushState_27706db1f99a6c87: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.pushState(arg1, getStringFromWasm0(arg2, arg3));
        }, arguments); },
        __wbg_pushState_f4bbfda83cdfe6c2: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.pushState(arg1, getStringFromWasm0(arg2, arg3), arg4 === 0 ? undefined : getStringFromWasm0(arg4, arg5));
        }, arguments); },
        __wbg_push_a6822215aa43e71c: function(arg0, arg1) {
            const ret = arg0.push(arg1);
            return ret;
        },
        __wbg_querySelector_47503871cb83f294: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.querySelector(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_querySelector_6f6509bf1f8f4753: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.querySelector(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_querySelector_862d54b2a67e9573: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.querySelector(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_queueMicrotask_35c611f4a14830b2: function(arg0) {
            queueMicrotask(arg0);
        },
        __wbg_queueMicrotask_404ed0a58e0b63cc: function(arg0) {
            const ret = arg0.queueMicrotask;
            return ret;
        },
        __wbg_queueMicrotask_ce275d8d3c6dfdf6: function(arg0, arg1) {
            arg0.queueMicrotask(arg1);
        },
        __wbg_race_b64ea4971405f526: function(arg0) {
            const ret = Promise.race(arg0);
            return ret;
        },
        __wbg_random_33cfffca5c784d5e: function() {
            const ret = Math.random();
            return ret;
        },
        __wbg_rangeOffset_3a0a3c62315130ca: function(arg0) {
            const ret = arg0.rangeOffset;
            return ret;
        },
        __wbg_rangeParent_73f34fbd10f112b2: function(arg0) {
            const ret = arg0.rangeParent;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_raw_0ef57339121466a2: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = String.raw(arg0, getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_raw_2cbe23f9c78a1fd4: function() { return handleError(function (arg0) {
            const ret = String.raw(arg0);
            return ret;
        }, arguments); },
        __wbg_raw_2cd9479419a6a47b: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            const ret = String.raw(arg0, getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
            return ret;
        }, arguments); },
        __wbg_raw_33918d41bdc47e8b: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            const ret = String.raw(arg0, getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
            return ret;
        }, arguments); },
        __wbg_raw_59d176a32442ff26: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            const ret = String.raw(arg0, getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
            return ret;
        }, arguments); },
        __wbg_raw_718b80e6a78adbcd: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            const ret = String.raw(arg0, getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
            return ret;
        }, arguments); },
        __wbg_raw_9ebbfafd23a65af1: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            const ret = String.raw(arg0, getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
            return ret;
        }, arguments); },
        __wbg_raw_d6c96fd4403f85e2: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = String.raw(arg0, getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
            return ret;
        }, arguments); },
        __wbg_raw_fc86ce7e25447ac2: function() { return handleError(function (arg0, arg1) {
            const ret = String.raw(arg0, ...arg1);
            return ret;
        }, arguments); },
        __wbg_readOnly_13f0673887c3c745: function(arg0) {
            const ret = arg0.readOnly;
            return ret;
        },
        __wbg_readOnly_44fbbbcbaa1d1a39: function(arg0) {
            const ret = arg0.readOnly;
            return ret;
        },
        __wbg_readyState_490503c1fa8f8dd6: function(arg0) {
            const ret = arg0.readyState;
            return ret;
        },
        __wbg_readyState_ba685ade827ebe84: function(arg0, arg1) {
            const ret = arg1.readyState;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_reason_4624d424a130e5b2: function(arg0, arg1) {
            const ret = arg1.reason;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_redirect_b99e2a4be63aa6a0: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Response.redirect(getStringFromWasm0(arg0, arg1), arg2);
            return ret;
        }, arguments); },
        __wbg_redirect_f33552a662e0df01: function() { return handleError(function (arg0, arg1) {
            const ret = Response.redirect(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_redirected_57ac695964bff80d: function(arg0) {
            const ret = arg0.redirected;
            return ret;
        },
        __wbg_referrerPolicy_273b42553d3c9f49: function(arg0, arg1) {
            const ret = arg1.referrerPolicy;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_referrerPolicy_6bcc9c344ca6aaea: function(arg0, arg1) {
            const ret = arg1.referrerPolicy;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_referrer_6609b952519a9afa: function(arg0, arg1) {
            const ret = arg1.referrer;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_region_3bf98aa8277adac4: function(arg0, arg1) {
            const ret = arg1.region;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_region_4576608bb24e6d6e: function(arg0) {
            const ret = arg0.region;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_registerContentHandler_43870df6f07ff181: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.registerContentHandler(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_registerProtocolHandler_8e8f0ac2f91253fa: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.registerProtocolHandler(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_register_2b38856410c0745e: function(arg0, arg1, arg2) {
            arg0.register(arg1, arg2);
        },
        __wbg_register_616541bf5c11f4e0: function(arg0, arg1, arg2, arg3) {
            arg0.register(arg1, arg2, arg3);
        },
        __wbg_reject_5cfff571450086dd: function(arg0) {
            const ret = Promise.reject(arg0);
            return ret;
        },
        __wbg_rel_5bb522c7f58b4ce0: function(arg0, arg1) {
            const ret = arg1.rel;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_relatedTarget_46131fb680472718: function(arg0) {
            const ret = arg0.relatedTarget;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_releaseCapture_448f515232e3f045: function(arg0) {
            arg0.releaseCapture();
        },
        __wbg_releaseCapture_c138b1fc2302fa7f: function(arg0) {
            arg0.releaseCapture();
        },
        __wbg_releaseEvents_d6e90badeb2b0da4: function(arg0) {
            arg0.releaseEvents();
        },
        __wbg_releasePointerCapture_854523512ca9059a: function() { return handleError(function (arg0, arg1) {
            arg0.releasePointerCapture(arg1);
        }, arguments); },
        __wbg_reload_0171f5b441317ac5: function() { return handleError(function (arg0) {
            arg0.reload();
        }, arguments); },
        __wbg_reload_898363a462f7015e: function() { return handleError(function (arg0, arg1) {
            arg0.reload(arg1 !== 0);
        }, arguments); },
        __wbg_removeAttributeNS_9f95fd9981b7f95b: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.removeAttributeNS(arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_removeAttribute_b89a6faf3f810f84: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.removeAttribute(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_removeChild_542662c726ba0cbf: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.removeChild(arg1);
            return ret;
        }, arguments); },
        __wbg_removeEventListener_5f35962e6c0b2ddc: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.removeEventListener(getStringFromWasm0(arg1, arg2), arg3);
        }, arguments); },
        __wbg_removeEventListener_9e2e49dbe3ca4858: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.removeEventListener(getStringFromWasm0(arg1, arg2), arg3, arg4 !== 0);
        }, arguments); },
        __wbg_removeListener_51d4a4f3fe03f9f3: function() { return handleError(function (arg0, arg1) {
            arg0.removeListener(arg1);
        }, arguments); },
        __wbg_removeProperty_d208a45736ad36a4: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg1.removeProperty(getStringFromWasm0(arg2, arg3));
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_remove_e74641df7d14f173: function(arg0) {
            arg0.remove();
        },
        __wbg_remove_f3fdf26d49490e53: function(arg0) {
            arg0.remove();
        },
        __wbg_repeat_136a627581271bd3: function(arg0) {
            const ret = arg0.repeat;
            return ret;
        },
        __wbg_repeat_fdc91b1bc8142819: function(arg0, arg1) {
            const ret = arg0.repeat(arg1);
            return ret;
        },
        __wbg_replaceAll_329af45069960b8e: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.replaceAll(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        },
        __wbg_replaceAll_3d8fdc760b21bc2c: function(arg0, arg1, arg2) {
            const ret = arg0.replaceAll(arg1, arg2);
            return ret;
        },
        __wbg_replaceAll_595f4ac404add82d: function(arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.replaceAll(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
            return ret;
        },
        __wbg_replaceAll_f16e02332dc06e8e: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.replaceAll(arg1, getStringFromWasm0(arg2, arg3));
            return ret;
        },
        __wbg_replaceChild_d525cded560578a9: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.replaceChild(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_replaceChildren_02cd41f40866642b: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.replaceChildren(arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_replaceChildren_06c4fb6e9397d0ea: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        },
        __wbg_replaceChildren_07a78509a4293ce1: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.replaceChildren(arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_replaceChildren_07fd4cc0d671c7c6: function(arg0, arg1) {
            arg0.replaceChildren(...arg1);
        },
        __wbg_replaceChildren_127cca2000a90838: function(arg0, arg1, arg2, arg3) {
            arg0.replaceChildren(arg1, arg2, arg3);
        },
        __wbg_replaceChildren_1322b7298657f9d8: function(arg0, arg1) {
            arg0.replaceChildren(...arg1);
        },
        __wbg_replaceChildren_16734c82252ddb94: function(arg0) {
            arg0.replaceChildren();
        },
        __wbg_replaceChildren_17b8feb448b66caa: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        },
        __wbg_replaceChildren_1b0babb8825432c8: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        },
        __wbg_replaceChildren_1cb08f3bffb86792: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        },
        __wbg_replaceChildren_1cffb4b11772978f: function(arg0, arg1) {
            arg0.replaceChildren(arg1);
        },
        __wbg_replaceChildren_1d157b143bcb7c2d: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.replaceChildren(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        },
        __wbg_replaceChildren_22c4bd7b8124c632: function(arg0, arg1, arg2, arg3) {
            arg0.replaceChildren(arg1, arg2, arg3);
        },
        __wbg_replaceChildren_249a94e926f1e292: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        },
        __wbg_replaceChildren_2fd393c7e1b4feae: function(arg0, arg1, arg2, arg3) {
            arg0.replaceChildren(arg1, arg2, arg3);
        },
        __wbg_replaceChildren_37f76315149983d7: function(arg0, arg1) {
            arg0.replaceChildren(...arg1);
        },
        __wbg_replaceChildren_3856e45be4715aa0: function(arg0, arg1, arg2) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2));
        },
        __wbg_replaceChildren_38905c15951e723c: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        },
        __wbg_replaceChildren_3cebfd3371af69ba: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        },
        __wbg_replaceChildren_3fce4047a747415b: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        },
        __wbg_replaceChildren_4b0409dad9e41f14: function(arg0, arg1) {
            arg0.replaceChildren(...arg1);
        },
        __wbg_replaceChildren_4c7b6ba678a0c4c5: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.replaceChildren(arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_replaceChildren_54544293508512b1: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        },
        __wbg_replaceChildren_573e0062a286c00b: function(arg0, arg1, arg2) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2));
        },
        __wbg_replaceChildren_5db48224a3c3a79d: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        },
        __wbg_replaceChildren_5fbccac56141dbd4: function(arg0) {
            arg0.replaceChildren();
        },
        __wbg_replaceChildren_6133652e5a363396: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.replaceChildren(arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_replaceChildren_61e344c07ce1b0a1: function(arg0) {
            arg0.replaceChildren();
        },
        __wbg_replaceChildren_6d6155ea1f2933ca: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        },
        __wbg_replaceChildren_71af3d16e8513207: function(arg0, arg1, arg2) {
            arg0.replaceChildren(arg1, arg2);
        },
        __wbg_replaceChildren_75c534fc126eb46a: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        },
        __wbg_replaceChildren_761f567a325a3ba8: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.replaceChildren(arg1, arg2, arg3, arg4);
        },
        __wbg_replaceChildren_7e45df0e978a6617: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.replaceChildren(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        },
        __wbg_replaceChildren_84c22e155a51e196: function(arg0, arg1) {
            arg0.replaceChildren(arg1);
        },
        __wbg_replaceChildren_8aefd947154eb166: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.replaceChildren(arg1, arg2, arg3, arg4);
        },
        __wbg_replaceChildren_8c97dc8841a54cc2: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.replaceChildren(arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_replaceChildren_948415de05e96076: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        },
        __wbg_replaceChildren_9c508dfd693d111f: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        },
        __wbg_replaceChildren_a246f597d3a30c1e: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.replaceChildren(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        },
        __wbg_replaceChildren_a3293548796dec44: function(arg0, arg1) {
            arg0.replaceChildren(arg1);
        },
        __wbg_replaceChildren_a3ba9275d6d0a312: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.replaceChildren(arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_replaceChildren_a6e09b1c0c4505fd: function(arg0) {
            arg0.replaceChildren();
        },
        __wbg_replaceChildren_c00c0376700855b1: function(arg0) {
            arg0.replaceChildren();
        },
        __wbg_replaceChildren_c72db6574079c8d4: function(arg0, arg1, arg2) {
            arg0.replaceChildren(arg1, arg2);
        },
        __wbg_replaceChildren_cb7cc402a364a7c5: function(arg0, arg1) {
            arg0.replaceChildren(...arg1);
        },
        __wbg_replaceChildren_de30a4cab036f8a2: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        },
        __wbg_replaceChildren_e36c893a0ac88ad2: function(arg0, arg1, arg2) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2));
        },
        __wbg_replaceChildren_e4eac621e0c6dcfa: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.replaceChildren(arg1, arg2, arg3, arg4);
        },
        __wbg_replaceChildren_e68290292a8da90d: function(arg0, arg1) {
            arg0.replaceChildren(...arg1);
        },
        __wbg_replaceChildren_e6e8517703c323af: function(arg0) {
            arg0.replaceChildren();
        },
        __wbg_replaceChildren_ea048a2f41d18787: function(arg0, arg1, arg2) {
            arg0.replaceChildren(arg1, arg2);
        },
        __wbg_replaceChildren_ed815dbf1802ac08: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        },
        __wbg_replaceChildren_f49dd888fe806a65: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        },
        __wbg_replaceChildren_fc0dc98aa9282ecf: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.replaceChildren(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        },
        __wbg_replaceData_ec7ad78c83da8e69: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.replaceData(arg1 >>> 0, arg2 >>> 0, getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_replaceState_827056d3e0653693: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.replaceState(arg1, getStringFromWasm0(arg2, arg3), arg4 === 0 ? undefined : getStringFromWasm0(arg4, arg5));
        }, arguments); },
        __wbg_replaceState_891e1d8923ee9136: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.replaceState(arg1, getStringFromWasm0(arg2, arg3));
        }, arguments); },
        __wbg_replaceSync_f9f66ec1558275df: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.replaceSync(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_replaceWith_1ea40119b0da11a7: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        }, arguments); },
        __wbg_replaceWith_269977c51ae8db18: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.replaceWith(arg1, arg2, arg3, arg4);
        }, arguments); },
        __wbg_replaceWith_28fc4a5a992e452c: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12, arg13, arg14) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12), getStringFromWasm0(arg13, arg14));
        }, arguments); },
        __wbg_replaceWith_39c53ea86e04d10d: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_replaceWith_39fc597c1f288f71: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_replaceWith_42fe4708ca9a04ab: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.replaceWith(arg1, arg2, arg3, arg4, arg5, arg6);
        }, arguments); },
        __wbg_replaceWith_48ab1a39a837e237: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.replaceWith(arg1, arg2, arg3);
        }, arguments); },
        __wbg_replaceWith_4a0ffe97322b9657: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        }, arguments); },
        __wbg_replaceWith_4ad71cdfc72810ee: function() { return handleError(function (arg0, arg1) {
            arg0.replaceWith(...arg1);
        }, arguments); },
        __wbg_replaceWith_55529dfd940a2a3e: function() { return handleError(function (arg0, arg1) {
            arg0.replaceWith(arg1);
        }, arguments); },
        __wbg_replaceWith_58f32544a157360c: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        }, arguments); },
        __wbg_replaceWith_5e79d11f03e739c6: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.replaceWith(arg1, arg2);
        }, arguments); },
        __wbg_replaceWith_773ea56cb1233f96: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        }, arguments); },
        __wbg_replaceWith_782b436e1e9eeff3: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_replaceWith_7a39a3853d6c6208: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_replaceWith_8496bcf423baf71b: function() { return handleError(function (arg0, arg1) {
            arg0.replaceWith(...arg1);
        }, arguments); },
        __wbg_replaceWith_8787ec4a83cccfa7: function() { return handleError(function (arg0) {
            arg0.replaceWith();
        }, arguments); },
        __wbg_replaceWith_8be93b383944eeaa: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10));
        }, arguments); },
        __wbg_replaceWith_9835d911229b258f: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.replaceWith(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        }, arguments); },
        __wbg_replaceWith_9bb7264e0d56344a: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.replaceWith(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_replaceWith_a311e7b280579377: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_replaceWith_a6ce7f36aec50557: function() { return handleError(function (arg0, arg1) {
            arg0.replaceWith(...arg1);
        }, arguments); },
        __wbg_replaceWith_a9b4ae769681166e: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.replaceWith(arg1, arg2, arg3, arg4);
        }, arguments); },
        __wbg_replaceWith_b07f21f8e4d4e1fa: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_replaceWith_b44b580ae9cef942: function() { return handleError(function (arg0) {
            arg0.replaceWith();
        }, arguments); },
        __wbg_replaceWith_b494a6539c540952: function() { return handleError(function (arg0, arg1) {
            arg0.replaceWith(...arg1);
        }, arguments); },
        __wbg_replaceWith_bbfef6b981bd26a8: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8));
        }, arguments); },
        __wbg_replaceWith_bc995f989dbaefd3: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.replaceWith(arg1, arg2, arg3);
        }, arguments); },
        __wbg_replaceWith_c82c73d1e81e6267: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.replaceWith(arg1, arg2, arg3, arg4, arg5, arg6);
        }, arguments); },
        __wbg_replaceWith_ce34eb0a97d43613: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.replaceWith(arg1, arg2);
        }, arguments); },
        __wbg_replaceWith_d0058aa0bad1366b: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            arg0.replaceWith(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
        }, arguments); },
        __wbg_replaceWith_d7eaa61f1257498d: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10, arg11, arg12) {
            arg0.replaceWith(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6), getStringFromWasm0(arg7, arg8), getStringFromWasm0(arg9, arg10), getStringFromWasm0(arg11, arg12));
        }, arguments); },
        __wbg_replaceWith_de30e0e9091e69f5: function() { return handleError(function (arg0, arg1) {
            arg0.replaceWith(arg1);
        }, arguments); },
        __wbg_replaceWith_e8cc0fea3930f5d5: function() { return handleError(function (arg0) {
            arg0.replaceWith();
        }, arguments); },
        __wbg_replaceWith_f856bdfaf0e81bfa: function() { return handleError(function (arg0) {
            arg0.replaceWith();
        }, arguments); },
        __wbg_replaceWith_f9279b5bae09c0a2: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.replaceWith(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_replace_2575f318b843da44: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.replace(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        },
        __wbg_replace_45abd521188a13b3: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.replace(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_replace_9028fd89d7761c5c: function(arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.replace(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
            return ret;
        },
        __wbg_replace_aacdcf2263939dea: function(arg0, arg1, arg2) {
            const ret = arg0.replace(arg1, arg2);
            return ret;
        },
        __wbg_replace_b8fab3c51cbdec7e: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.replace(arg1, getStringFromWasm0(arg2, arg3));
            return ret;
        },
        __wbg_replace_c9f84ade3c6f38d7: function() {
            const ret = Symbol.replace;
            return ret;
        },
        __wbg_replace_d3fc6af9fbc75204: function(arg0, arg1, arg2) {
            const ret = arg0.replace(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_reportValidity_70f93b8437d89c1e: function(arg0) {
            const ret = arg0.reportValidity();
            return ret;
        },
        __wbg_reportValidity_bac8315d1073904c: function(arg0) {
            const ret = arg0.reportValidity();
            return ret;
        },
        __wbg_requestAnimationFrame_72bbc2f340fc7a29: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.requestAnimationFrame(arg1);
            return ret;
        }, arguments); },
        __wbg_requestFullscreen_443e2134949cd116: function() { return handleError(function (arg0) {
            arg0.requestFullscreen();
        }, arguments); },
        __wbg_requestIdleCallback_c20526bc0acc80bc: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.requestIdleCallback(arg1);
            return ret;
        }, arguments); },
        __wbg_requestMIDIAccess_196a0b7e8cc29374: function() { return handleError(function (arg0) {
            const ret = arg0.requestMIDIAccess();
            return ret;
        }, arguments); },
        __wbg_requestMediaKeySystemAccess_720a96f6ace6340c: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.requestMediaKeySystemAccess(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        },
        __wbg_requestPointerLock_23fb5ef9c0cb58a9: function(arg0) {
            arg0.requestPointerLock();
        },
        __wbg_required_02bbd48afeda04dc: function(arg0) {
            const ret = arg0.required;
            return ret;
        },
        __wbg_required_c0738fcb4fab320c: function(arg0) {
            const ret = arg0.required;
            return ret;
        },
        __wbg_resizable_d1bb7592567064c7: function(arg0) {
            const ret = arg0.resizable;
            return ret;
        },
        __wbg_resizeBy_fd5c3e89cfbe20ba: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.resizeBy(arg1, arg2);
        }, arguments); },
        __wbg_resizeTo_8e1b010dd8fe4f75: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.resizeTo(arg1, arg2);
        }, arguments); },
        __wbg_resize_e9f67cb4d4d32e11: function() { return handleError(function (arg0, arg1) {
            arg0.resize(arg1 >>> 0);
        }, arguments); },
        __wbg_resolve_25a7e548d5881dca: function(arg0) {
            const ret = Promise.resolve(arg0);
            return ret;
        },
        __wbg_resolvedOptions_054c42ae9c837688: function(arg0) {
            const ret = arg0.resolvedOptions();
            return ret;
        },
        __wbg_resolvedOptions_17e26d86e7dbc0bb: function(arg0) {
            const ret = arg0.resolvedOptions();
            return ret;
        },
        __wbg_resolvedOptions_2da18e2f5d3309f3: function(arg0) {
            const ret = arg0.resolvedOptions();
            return ret;
        },
        __wbg_resolvedOptions_3139234787ec811f: function(arg0) {
            const ret = arg0.resolvedOptions();
            return ret;
        },
        __wbg_resolvedOptions_41d8c6f39ce3e29a: function(arg0) {
            const ret = arg0.resolvedOptions();
            return ret;
        },
        __wbg_resolvedOptions_437b3cc942db0f00: function(arg0) {
            const ret = arg0.resolvedOptions();
            return ret;
        },
        __wbg_resolvedOptions_d3b945b2caccfcaa: function(arg0) {
            const ret = arg0.resolvedOptions();
            return ret;
        },
        __wbg_resolvedOptions_d7a1fa53605d1322: function(arg0) {
            const ret = arg0.resolvedOptions();
            return ret;
        },
        __wbg_resolvedOptions_d9f0852dbc8d945a: function(arg0) {
            const ret = arg0.resolvedOptions();
            return ret;
        },
        __wbg_rev_58da06bbf2dd2e5d: function(arg0, arg1) {
            const ret = arg1.rev;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_revocable_66409b06457d8cc3: function(arg0, arg1) {
            const ret = Proxy.revocable(arg0, arg1);
            return ret;
        },
        __wbg_revokeObjectURL_02f29532cbc52b60: function() { return handleError(function (arg0, arg1) {
            URL.revokeObjectURL(getStringFromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_rightContext_8fe8ca7bde92e84c: function() {
            const ret = RegExp.rightContext;
            return ret;
        },
        __wbg_right_c5d17f2dff9d0d06: function(arg0) {
            const ret = arg0.right;
            return ret;
        },
        __wbg_round_9ca185967d3626e1: function(arg0) {
            const ret = Math.round(arg0);
            return ret;
        },
        __wbg_rows_a90f46e5d8873e68: function(arg0) {
            const ret = arg0.rows;
            return ret;
        },
        __wbg_screenX_36e450fb883d863c: function() { return handleError(function (arg0) {
            const ret = arg0.screenX;
            return ret;
        }, arguments); },
        __wbg_screenX_d72df4af68a5e385: function(arg0) {
            const ret = arg0.screenX;
            return ret;
        },
        __wbg_screenY_aa5662ddfe8bd0b3: function(arg0) {
            const ret = arg0.screenY;
            return ret;
        },
        __wbg_screenY_ed40ca346859458d: function() { return handleError(function (arg0) {
            const ret = arg0.screenY;
            return ret;
        }, arguments); },
        __wbg_script_450f86d97ac0b2ca: function(arg0) {
            const ret = arg0.script;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_scrollBy_245d946f4a92021d: function(arg0, arg1, arg2) {
            arg0.scrollBy(arg1, arg2);
        },
        __wbg_scrollBy_4b322381d586648f: function(arg0, arg1, arg2) {
            arg0.scrollBy(arg1, arg2);
        },
        __wbg_scrollBy_81ab567aed8fa927: function(arg0) {
            arg0.scrollBy();
        },
        __wbg_scrollBy_9f4b8ef3368038e1: function(arg0) {
            arg0.scrollBy();
        },
        __wbg_scrollHeight_28eca057d00b2962: function(arg0) {
            const ret = arg0.scrollHeight;
            return ret;
        },
        __wbg_scrollHeight_2c87ecaf755229ee: function(arg0) {
            const ret = arg0.scrollHeight;
            return ret;
        },
        __wbg_scrollIntoView_781000b96ece5193: function(arg0, arg1) {
            arg0.scrollIntoView(arg1 !== 0);
        },
        __wbg_scrollIntoView_82656b609b476691: function(arg0) {
            arg0.scrollIntoView();
        },
        __wbg_scrollLeft_7897f8332705d017: function(arg0) {
            const ret = arg0.scrollLeft;
            return ret;
        },
        __wbg_scrollTo_19d452fe95f09139: function(arg0) {
            arg0.scrollTo();
        },
        __wbg_scrollTo_4de01c6332daece3: function(arg0) {
            arg0.scrollTo();
        },
        __wbg_scrollTo_76d88a4c0481b97b: function(arg0, arg1, arg2) {
            arg0.scrollTo(arg1, arg2);
        },
        __wbg_scrollTo_a7933142b05c0233: function(arg0, arg1, arg2) {
            arg0.scrollTo(arg1, arg2);
        },
        __wbg_scrollTop_705ef336e30f2d3c: function(arg0) {
            const ret = arg0.scrollTop;
            return ret;
        },
        __wbg_scrollTop_a7ae07fae76663e3: function(arg0) {
            const ret = arg0.scrollTop;
            return ret;
        },
        __wbg_scrollWidth_48bec0be1eaedf88: function(arg0) {
            const ret = arg0.scrollWidth;
            return ret;
        },
        __wbg_scrollX_9c3fa310cb5ed346: function() { return handleError(function (arg0) {
            const ret = arg0.scrollX;
            return ret;
        }, arguments); },
        __wbg_scrollY_5502bd75aa44fd39: function() { return handleError(function (arg0) {
            const ret = arg0.scrollY;
            return ret;
        }, arguments); },
        __wbg_scroll_58cee1d5ef4aad5b: function(arg0) {
            arg0.scroll();
        },
        __wbg_scroll_8de4cab6525b5ecc: function(arg0) {
            arg0.scroll();
        },
        __wbg_scroll_90c2f08a012dd1a6: function(arg0, arg1, arg2) {
            arg0.scroll(arg1, arg2);
        },
        __wbg_scroll_ab64e589b335fde8: function(arg0, arg1, arg2) {
            arg0.scroll(arg1, arg2);
        },
        __wbg_scrollingElement_8438a951972949d2: function(arg0) {
            const ret = arg0.scrollingElement;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_scrolling_c871fafe7372ab81: function(arg0, arg1) {
            const ret = arg1.scrolling;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_search_38738e5c6e4d0cc3: function(arg0, arg1) {
            const ret = arg0.search(arg1);
            return ret;
        },
        __wbg_search_43f1ec3f13320cd0: function(arg0, arg1) {
            const ret = arg1.search;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_search_6ca760d7acd2e6e8: function() {
            const ret = Symbol.search;
            return ret;
        },
        __wbg_search_8b96c939223a26e5: function(arg0, arg1) {
            const ret = arg1.search;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_search_c299eeae1d9b169e: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.search;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_seconds_7684f93ebc0c6899: function(arg0, arg1) {
            const ret = arg1.seconds;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_segment_2205eb61b41aa51e: function(arg0) {
            const ret = arg0.segment;
            return ret;
        },
        __wbg_segment_e5a603d5c6c36599: function(arg0, arg1, arg2) {
            const ret = arg0.segment(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_selectRange_32dbf851a2f67d07: function(arg0, arg1, arg2) {
            const ret = arg0.selectRange(arg1, arg2);
            return ret;
        },
        __wbg_select_825b7186a349cb10: function(arg0) {
            arg0.select();
        },
        __wbg_select_c8b25221a62e4f2a: function(arg0, arg1) {
            const ret = arg0.select(arg1);
            return ret;
        },
        __wbg_select_eb5fe194807ca310: function(arg0) {
            arg0.select();
        },
        __wbg_selectedStyleSheetSet_c5db92329aec1066: function(arg0, arg1) {
            const ret = arg1.selectedStyleSheetSet;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_selectionDirection_7861578fee24212d: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.selectionDirection;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_selectionDirection_b918f72c25a232f3: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.selectionDirection;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_selectionEnd_4542675881c85550: function() { return handleError(function (arg0) {
            const ret = arg0.selectionEnd;
            return isLikeNone(ret) ? Number.MAX_SAFE_INTEGER : (ret) >>> 0;
        }, arguments); },
        __wbg_selectionEnd_86322b8fe75fc8ce: function() { return handleError(function (arg0) {
            const ret = arg0.selectionEnd;
            return isLikeNone(ret) ? Number.MAX_SAFE_INTEGER : (ret) >>> 0;
        }, arguments); },
        __wbg_selectionStart_890975ad75a68ef7: function() { return handleError(function (arg0) {
            const ret = arg0.selectionStart;
            return isLikeNone(ret) ? Number.MAX_SAFE_INTEGER : (ret) >>> 0;
        }, arguments); },
        __wbg_selectionStart_f2c4abdc434f9f22: function() { return handleError(function (arg0) {
            const ret = arg0.selectionStart;
            return isLikeNone(ret) ? Number.MAX_SAFE_INTEGER : (ret) >>> 0;
        }, arguments); },
        __wbg_selectorText_ef240d3269fe1060: function(arg0, arg1) {
            const ret = arg1.selectorText;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_self_f96d4c98714978d5: function(arg0) {
            const ret = arg0.self;
            return ret;
        },
        __wbg_sendBeacon_0c49b42430353056: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.sendBeacon(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_sendBeacon_1013da90c9568341: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.sendBeacon(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        }, arguments); },
        __wbg_sendBeacon_460899c7850e25d1: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.sendBeacon(getStringFromWasm0(arg1, arg2), arg3 === 0 ? undefined : getArrayU8FromWasm0(arg3, arg4));
            return ret;
        }, arguments); },
        __wbg_sendBeacon_e10b6641d7add970: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.sendBeacon(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        }, arguments); },
        __wbg_sendBeacon_f5e0e543c418b9d1: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.sendBeacon(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        }, arguments); },
        __wbg_sendBeacon_f7b8b5a89dd3f75e: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.sendBeacon(getStringFromWasm0(arg1, arg2), arg3 === 0 ? undefined : getStringFromWasm0(arg3, arg4));
            return ret;
        }, arguments); },
        __wbg_send_057c26ad18c5735a: function() { return handleError(function (arg0, arg1) {
            arg0.send(arg1);
        }, arguments); },
        __wbg_send_1a72c4a9aa027ddc: function() { return handleError(function (arg0, arg1) {
            arg0.send(arg1);
        }, arguments); },
        __wbg_send_1ffa2ec856c1d570: function() { return handleError(function (arg0, arg1) {
            arg0.send(arg1);
        }, arguments); },
        __wbg_send_2da729b2f85d2e8a: function() { return handleError(function (arg0, arg1) {
            arg0.send(arg1);
        }, arguments); },
        __wbg_send_35647f35f8bdac5d: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.send(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_send_4a773f523104d75e: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.send(getArrayU8FromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_setAttributeNS_d688c13aabdefeed: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setAttributeNS(arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_setAttribute_5b695d1c3be2e3e6: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.setAttribute(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_setCapture_4fe6c9660e2ca1aa: function(arg0, arg1) {
            arg0.setCapture(arg1 !== 0);
        },
        __wbg_setCapture_fa35bfe3692f4d42: function(arg0) {
            arg0.setCapture();
        },
        __wbg_setCustomValidity_23d0cf814425b87f: function(arg0, arg1, arg2) {
            arg0.setCustomValidity(getStringFromWasm0(arg1, arg2));
        },
        __wbg_setCustomValidity_8bb8de0abebcf09d: function(arg0, arg1, arg2) {
            arg0.setCustomValidity(getStringFromWasm0(arg1, arg2));
        },
        __wbg_setDate_3265acef72c43eb1: function(arg0, arg1) {
            const ret = arg0.setDate(arg1 >>> 0);
            return ret;
        },
        __wbg_setFloat16_068a7ce2ae0ab079: function(arg0, arg1, arg2) {
            arg0.setFloat16(arg1 >>> 0, arg2);
        },
        __wbg_setFloat16_8d3520e4455ecd07: function(arg0, arg1, arg2, arg3) {
            arg0.setFloat16(arg1 >>> 0, arg2, arg3 !== 0);
        },
        __wbg_setFloat32_6522aac93e8193ac: function(arg0, arg1, arg2, arg3) {
            arg0.setFloat32(arg1 >>> 0, arg2, arg3 !== 0);
        },
        __wbg_setFloat32_975b076aea843f30: function(arg0, arg1, arg2) {
            arg0.setFloat32(arg1 >>> 0, arg2);
        },
        __wbg_setFloat64_5750c7625c7e5215: function(arg0, arg1, arg2) {
            arg0.setFloat64(arg1 >>> 0, arg2);
        },
        __wbg_setFloat64_c2abf164d01c7462: function(arg0, arg1, arg2, arg3) {
            arg0.setFloat64(arg1 >>> 0, arg2, arg3 !== 0);
        },
        __wbg_setFullYear_8c45edfedfbd3eff: function(arg0, arg1) {
            const ret = arg0.setFullYear(arg1 >>> 0);
            return ret;
        },
        __wbg_setFullYear_9bf375484c5ae30f: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.setFullYear(arg1 >>> 0, arg2, arg3);
            return ret;
        },
        __wbg_setFullYear_bd4a83f1ffaea598: function(arg0, arg1, arg2) {
            const ret = arg0.setFullYear(arg1 >>> 0, arg2);
            return ret;
        },
        __wbg_setHours_ebc8628c414f9959: function(arg0, arg1) {
            const ret = arg0.setHours(arg1 >>> 0);
            return ret;
        },
        __wbg_setInt16_a07f08ae91841e6d: function(arg0, arg1, arg2, arg3) {
            arg0.setInt16(arg1 >>> 0, arg2, arg3 !== 0);
        },
        __wbg_setInt16_f1645ac744dd3681: function(arg0, arg1, arg2) {
            arg0.setInt16(arg1 >>> 0, arg2);
        },
        __wbg_setInt32_19bcff712444ab4e: function(arg0, arg1, arg2) {
            arg0.setInt32(arg1 >>> 0, arg2);
        },
        __wbg_setInt32_9d37f8b0841c7b07: function(arg0, arg1, arg2, arg3) {
            arg0.setInt32(arg1 >>> 0, arg2, arg3 !== 0);
        },
        __wbg_setInt8_8b06e083d2ddb1f5: function(arg0, arg1, arg2) {
            arg0.setInt8(arg1 >>> 0, arg2);
        },
        __wbg_setInterval_0aaae807524b734c: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            const ret = arg0.setInterval(getStringFromWasm0(arg1, arg2), arg3, arg4, arg5, arg6);
            return ret;
        }, arguments); },
        __wbg_setInterval_12b5192a3411feb2: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            const ret = arg0.setInterval(arg1, arg2, arg3, arg4, arg5);
            return ret;
        }, arguments); },
        __wbg_setInterval_19642995e837d2ee: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.setInterval(getStringFromWasm0(arg1, arg2), arg3, ...arg4);
            return ret;
        }, arguments); },
        __wbg_setInterval_25b931a556cd3104: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            const ret = arg0.setInterval(getStringFromWasm0(arg1, arg2), arg3, arg4, arg5, arg6, arg7);
            return ret;
        }, arguments); },
        __wbg_setInterval_3b414ba9de2cfd29: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            const ret = arg0.setInterval(arg1, arg2, arg3, arg4, arg5, arg6);
            return ret;
        }, arguments); },
        __wbg_setInterval_415d568f91ab2fab: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.setInterval(arg1, arg2, arg3, arg4);
            return ret;
        }, arguments); },
        __wbg_setInterval_5452cfde69053cf6: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.setInterval(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        }, arguments); },
        __wbg_setInterval_5e998c88967236d2: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            const ret = arg0.setInterval(arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9);
            return ret;
        }, arguments); },
        __wbg_setInterval_7d036e28155f28a9: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            const ret = arg0.setInterval(arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8);
            return ret;
        }, arguments); },
        __wbg_setInterval_7d26a0f56ff0e5d9: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            const ret = arg0.setInterval(getStringFromWasm0(arg1, arg2), arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10);
            return ret;
        }, arguments); },
        __wbg_setInterval_8dafa5d1d8549fad: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            const ret = arg0.setInterval(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
            return ret;
        }, arguments); },
        __wbg_setInterval_a60d3e6b477aed2b: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            const ret = arg0.setInterval(getStringFromWasm0(arg1, arg2), arg3, arg4, arg5);
            return ret;
        }, arguments); },
        __wbg_setInterval_ae27514ca815a798: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.setInterval(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_setInterval_ca11d4274762d64e: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.setInterval(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_setInterval_ca9326aa60005c95: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.setInterval(getStringFromWasm0(arg1, arg2), arg3, arg4);
            return ret;
        }, arguments); },
        __wbg_setInterval_cb294dac9789d9b5: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            const ret = arg0.setInterval(getStringFromWasm0(arg1, arg2), arg3, arg4, arg5, arg6, arg7, arg8);
            return ret;
        }, arguments); },
        __wbg_setInterval_cf411a8675fd0b9e: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.setInterval(arg1, arg2, arg3);
            return ret;
        }, arguments); },
        __wbg_setInterval_d28117a00581428c: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            const ret = arg0.setInterval(getStringFromWasm0(arg1, arg2), arg3, arg4, arg5, arg6, arg7, arg8, arg9);
            return ret;
        }, arguments); },
        __wbg_setInterval_d41afd5dcd651fce: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.setInterval(arg1, arg2, ...arg3);
            return ret;
        }, arguments); },
        __wbg_setInterval_d56eadba95805b6e: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.setInterval(arg1);
            return ret;
        }, arguments); },
        __wbg_setMilliseconds_761d157329e4031a: function(arg0, arg1) {
            const ret = arg0.setMilliseconds(arg1 >>> 0);
            return ret;
        },
        __wbg_setMinutes_66043b71ef7e29c7: function(arg0, arg1) {
            const ret = arg0.setMinutes(arg1 >>> 0);
            return ret;
        },
        __wbg_setMonth_ff4c0a3f404c2bfd: function(arg0, arg1) {
            const ret = arg0.setMonth(arg1 >>> 0);
            return ret;
        },
        __wbg_setPointerCapture_306526a07972683e: function() { return handleError(function (arg0, arg1) {
            arg0.setPointerCapture(arg1);
        }, arguments); },
        __wbg_setProperty_a6e0b14612e307b1: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.setProperty(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_setProperty_b21c8c7e36721268: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setProperty(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_setRangeText_1766e48470bde30e: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.setRangeText(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_setRangeText_7d61b7d488c58005: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.setRangeText(getStringFromWasm0(arg1, arg2), arg3 >>> 0, arg4 >>> 0);
        }, arguments); },
        __wbg_setRangeText_a75e0aec2a9c6f05: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setRangeText(getStringFromWasm0(arg1, arg2), arg3 >>> 0, arg4 >>> 0, getStringFromWasm0(arg5, arg6));
        }, arguments); },
        __wbg_setRangeText_b689b14748f1a35a: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.setRangeText(getStringFromWasm0(arg1, arg2), arg3 >>> 0, arg4 >>> 0);
        }, arguments); },
        __wbg_setRangeText_f6ef98f37143df35: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.setRangeText(getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_setResourceTimingBufferSize_94e9ae5aa0f839a2: function(arg0, arg1) {
            arg0.setResourceTimingBufferSize(arg1 >>> 0);
        },
        __wbg_setSeconds_cd75c792658aa4ce: function(arg0, arg1) {
            const ret = arg0.setSeconds(arg1 >>> 0);
            return ret;
        },
        __wbg_setSelectionRange_3cda579df98e5508: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.setSelectionRange(arg1 >>> 0, arg2 >>> 0);
        }, arguments); },
        __wbg_setSelectionRange_470fd6fd30578d45: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.setSelectionRange(arg1 >>> 0, arg2 >>> 0);
        }, arguments); },
        __wbg_setSelectionRange_6fff82da5bc94c44: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.setSelectionRange(arg1 >>> 0, arg2 >>> 0, getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_setSelectionRange_d86256490f6ea034: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.setSelectionRange(arg1 >>> 0, arg2 >>> 0, getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_setTime_182c8306c0d12d6f: function(arg0, arg1) {
            const ret = arg0.setTime(arg1);
            return ret;
        },
        __wbg_setTimeout_0846a80857c4c18f: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            const ret = arg0.setTimeout(getStringFromWasm0(arg1, arg2), arg3, arg4, arg5, arg6, arg7, arg8, arg9);
            return ret;
        }, arguments); },
        __wbg_setTimeout_2237ac5dda3467ae: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.setTimeout(arg1);
            return ret;
        }, arguments); },
        __wbg_setTimeout_3dd06ce5eae72c5f: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            const ret = arg0.setTimeout(getStringFromWasm0(arg1, arg2), arg3, arg4, arg5, arg6);
            return ret;
        }, arguments); },
        __wbg_setTimeout_442954c245cf5ba0: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.setTimeout(arg1, arg2, arg3, arg4);
            return ret;
        }, arguments); },
        __wbg_setTimeout_4b98ba20112031b8: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10) {
            const ret = arg0.setTimeout(getStringFromWasm0(arg1, arg2), arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10);
            return ret;
        }, arguments); },
        __wbg_setTimeout_5fdd767d81fe03ef: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) {
            const ret = arg0.setTimeout(arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9);
            return ret;
        }, arguments); },
        __wbg_setTimeout_6f1e0f55cd40619f: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            const ret = arg0.setTimeout(arg1, arg2, arg3, arg4, arg5, arg6);
            return ret;
        }, arguments); },
        __wbg_setTimeout_75f1cbb12b59f833: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.setTimeout(arg1, arg2, arg3);
            return ret;
        }, arguments); },
        __wbg_setTimeout_872521eeaa80dc22: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.setTimeout(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_setTimeout_8ef603eb138cb73e: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.setTimeout(arg1, arg2, ...arg3);
            return ret;
        }, arguments); },
        __wbg_setTimeout_91ecbd6703de90fe: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            const ret = arg0.setTimeout(arg1, arg2, arg3, arg4, arg5, arg6, arg7);
            return ret;
        }, arguments); },
        __wbg_setTimeout_98247319e6ff54a2: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            const ret = arg0.setTimeout(getStringFromWasm0(arg1, arg2), arg3, arg4, arg5);
            return ret;
        }, arguments); },
        __wbg_setTimeout_9eb50a8b76a86b7d: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            const ret = arg0.setTimeout(getStringFromWasm0(arg1, arg2), arg3, arg4, arg5, arg6, arg7);
            return ret;
        }, arguments); },
        __wbg_setTimeout_a200734d69b662ed: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.setTimeout(getStringFromWasm0(arg1, arg2), arg3, arg4);
            return ret;
        }, arguments); },
        __wbg_setTimeout_a3e4c3542e96272e: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.setTimeout(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        }, arguments); },
        __wbg_setTimeout_b5f25e402b6e8ff9: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.setTimeout(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_setTimeout_bc242c2bfc8e24e7: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            const ret = arg0.setTimeout(arg1, arg2, arg3, arg4, arg5);
            return ret;
        }, arguments); },
        __wbg_setTimeout_bd2694b07c6a3006: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            const ret = arg0.setTimeout(getStringFromWasm0(arg1, arg2), arg3, arg4, arg5, arg6, arg7, arg8);
            return ret;
        }, arguments); },
        __wbg_setTimeout_f672fc3c6b3db271: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.setTimeout(getStringFromWasm0(arg1, arg2), arg3, ...arg4);
            return ret;
        }, arguments); },
        __wbg_setTimeout_fe392a2e8f657697: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            const ret = arg0.setTimeout(arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8);
            return ret;
        }, arguments); },
        __wbg_setUTCDate_053f398a32c3fcc2: function(arg0, arg1) {
            const ret = arg0.setUTCDate(arg1 >>> 0);
            return ret;
        },
        __wbg_setUTCFullYear_189dbce506b19050: function(arg0, arg1, arg2) {
            const ret = arg0.setUTCFullYear(arg1 >>> 0, arg2);
            return ret;
        },
        __wbg_setUTCFullYear_5ec14306e786bcca: function(arg0, arg1) {
            const ret = arg0.setUTCFullYear(arg1 >>> 0);
            return ret;
        },
        __wbg_setUTCFullYear_cbd4274cb8378699: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.setUTCFullYear(arg1 >>> 0, arg2, arg3);
            return ret;
        },
        __wbg_setUTCHours_e8b71aa5389b2e26: function(arg0, arg1) {
            const ret = arg0.setUTCHours(arg1 >>> 0);
            return ret;
        },
        __wbg_setUTCMilliseconds_56d811ee653b7575: function(arg0, arg1) {
            const ret = arg0.setUTCMilliseconds(arg1 >>> 0);
            return ret;
        },
        __wbg_setUTCMinutes_e778cd046539ca49: function(arg0, arg1) {
            const ret = arg0.setUTCMinutes(arg1 >>> 0);
            return ret;
        },
        __wbg_setUTCMonth_6e37cd4cc16f3247: function(arg0, arg1) {
            const ret = arg0.setUTCMonth(arg1 >>> 0);
            return ret;
        },
        __wbg_setUTCSeconds_aa0c17ff3b4df3ba: function(arg0, arg1) {
            const ret = arg0.setUTCSeconds(arg1 >>> 0);
            return ret;
        },
        __wbg_setUint16_8d04482d4b142890: function(arg0, arg1, arg2) {
            arg0.setUint16(arg1 >>> 0, arg2);
        },
        __wbg_setUint16_ba62ed59c2b89bac: function(arg0, arg1, arg2, arg3) {
            arg0.setUint16(arg1 >>> 0, arg2, arg3 !== 0);
        },
        __wbg_setUint32_55857ee5f6a59484: function(arg0, arg1, arg2) {
            arg0.setUint32(arg1 >>> 0, arg2 >>> 0);
        },
        __wbg_setUint32_f7240e6399b4d170: function(arg0, arg1, arg2, arg3) {
            arg0.setUint32(arg1 >>> 0, arg2 >>> 0, arg3 !== 0);
        },
        __wbg_setUint8_83404ede5f289ad8: function(arg0, arg1, arg2) {
            arg0.setUint8(arg1 >>> 0, arg2);
        },
        __wbg_set_0430bf343a2f9e34: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_17b040da5ecb5861: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_190f84efe1abf2cc: function(arg0, arg1, arg2) {
            arg0.set(getArrayU32FromWasm0(arg1, arg2));
        },
        __wbg_set_1af28963e8051498: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_1b66cf83755d9a2b: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_set_29c99a8aac1c01e5: function(arg0, arg1, arg2) {
            arg0.set(getArrayU8FromWasm0(arg1, arg2));
        },
        __wbg_set_31da7b152de5161c: function(arg0, arg1, arg2) {
            arg0.set(getArrayI16FromWasm0(arg1, arg2));
        },
        __wbg_set_4a5134984584c232: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_5e28e82805cc9992: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_63b87f7c2d096ec3: function(arg0, arg1, arg2) {
            arg0.set(getArrayF64FromWasm0(arg1, arg2));
        },
        __wbg_set_63df9416b3a21013: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = Reflect.set(arg0, arg1, arg2, arg3);
            return ret;
        }, arguments); },
        __wbg_set_6b2948b6fe9d09b4: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_6e30c9374c26414c: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_set_840629905999faba: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.set(arg1 >>> 0, arg2);
        }, arguments); },
        __wbg_set_8745aaf86ce85d8b: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_8fd65ef1c82fce80: function(arg0, arg1, arg2) {
            arg0.set(getArrayU8FromWasm0(arg1, arg2));
        },
        __wbg_set_a07c3b8b72f558d3: function(arg0, arg1, arg2) {
            arg0.set(getArrayU16FromWasm0(arg1, arg2));
        },
        __wbg_set_a2b1b133a653d559: function(arg0, arg1, arg2) {
            arg0.set(getArrayI8FromWasm0(arg1, arg2));
        },
        __wbg_set_accept_9a460304177af2ac: function(arg0, arg1, arg2) {
            arg0.accept = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_accessKey_18d2eb0a2615c6bd: function(arg0, arg1, arg2) {
            arg0.accessKey = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_ad3bde048c79fe2b: function(arg0, arg1, arg2) {
            arg0.set(getArrayI32FromWasm0(arg1, arg2));
        },
        __wbg_set_adoptedStyleSheets_8a53293b142ecf3b: function(arg0, arg1) {
            arg0.adoptedStyleSheets = arg1;
        },
        __wbg_set_aef7acdf59b60929: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_align_9ddcf66699215a11: function(arg0, arg1, arg2) {
            arg0.align = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_align_d21763f0d7d0a67d: function(arg0, arg1, arg2) {
            arg0.align = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_allowFullscreen_3b84f0e167d5e9c1: function(arg0, arg1) {
            arg0.allowFullscreen = arg1 !== 0;
        },
        __wbg_set_allowPaymentRequest_2a34e477efc73c50: function(arg0, arg1) {
            arg0.allowPaymentRequest = arg1 !== 0;
        },
        __wbg_set_alt_4fd335525196679f: function(arg0, arg1, arg2) {
            arg0.alt = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_autocomplete_5c5fd00602572ce8: function(arg0, arg1, arg2) {
            arg0.autocomplete = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_autocomplete_ca4585ce6839d445: function(arg0, arg1, arg2) {
            arg0.autocomplete = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_autofocus_41163f8f0b75e46a: function(arg0, arg1) {
            arg0.autofocus = arg1 !== 0;
        },
        __wbg_set_autofocus_966788ab40737263: function(arg0, arg1) {
            arg0.autofocus = arg1 !== 0;
        },
        __wbg_set_autofocus_c99f5865577f880c: function() { return handleError(function (arg0, arg1) {
            arg0.autofocus = arg1 !== 0;
        }, arguments); },
        __wbg_set_bd7b6300364788fc: function(arg0, arg1, arg2) {
            arg0.set(getArrayF32FromWasm0(arg1, arg2));
        },
        __wbg_set_binaryType_41994c453b95bdd2: function(arg0, arg1) {
            arg0.binaryType = __wbindgen_enum_BinaryType[arg1];
        },
        __wbg_set_body_3abdabfd168d7aed: function(arg0, arg1) {
            arg0.body = arg1;
        },
        __wbg_set_c10ed3c42f313ae6: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_c775d84916be79ea: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_c9e307c0b6988450: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(arg0, arg1 >>> 0, arg2);
            return ret;
        }, arguments); },
        __wbg_set_calendar_459c34aa5e9f6401: function(arg0, arg1, arg2) {
            arg0.calendar = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_cancelBubble_f865b95097fcf214: function(arg0, arg1) {
            arg0.cancelBubble = arg1 !== 0;
        },
        __wbg_set_case_first_33415706786555cc: function(arg0, arg1) {
            arg0.caseFirst = __wbindgen_enum_CollatorCaseFirst[arg1];
        },
        __wbg_set_cause_3ca6c9a47bee6af0: function(arg0, arg1) {
            arg0.cause = arg1;
        },
        __wbg_set_cause_eeddc045292cafe8: function(arg0, arg1) {
            arg0.cause = arg1;
        },
        __wbg_set_charset_3ccb69854e3e9113: function(arg0, arg1, arg2) {
            arg0.charset = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_checked_5b19bac8e2a3d914: function(arg0, arg1) {
            arg0.checked = arg1 !== 0;
        },
        __wbg_set_className_764842f07bec5aba: function(arg0, arg1, arg2) {
            arg0.className = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_cols_682555281084df4b: function(arg0, arg1) {
            arg0.cols = arg1 >>> 0;
        },
        __wbg_set_compact_display_202c14036441f8f6: function(arg0, arg1) {
            arg0.compactDisplay = __wbindgen_enum_CompactDisplay[arg1];
        },
        __wbg_set_contentEditable_c5f25c3f6b77ffaf: function(arg0, arg1, arg2) {
            arg0.contentEditable = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_coords_1f1e902053dfbf34: function(arg0, arg1, arg2) {
            arg0.coords = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_cssText_30e57faf261e49e7: function(arg0, arg1, arg2) {
            arg0.cssText = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_cssText_e5b69577f3490884: function(arg0, arg1, arg2) {
            arg0.cssText = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_currency_0ce8c1a37eba5ab9: function(arg0, arg1, arg2) {
            arg0.currency = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_currency_display_485d0a769eb749a2: function(arg0, arg1) {
            arg0.currencyDisplay = __wbindgen_enum_CurrencyDisplay[arg1];
        },
        __wbg_set_currency_sign_de7df871ea128c18: function(arg0, arg1) {
            arg0.currencySign = __wbindgen_enum_CurrencySign[arg1];
        },
        __wbg_set_d5f2d8f2dbf49f84: function(arg0, arg1, arg2) {
            arg0.set(getArrayU64FromWasm0(arg1, arg2));
        },
        __wbg_set_data_52bc8ae1eb8bd677: function(arg0, arg1, arg2) {
            arg0.data = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_date_style_40b4166029ec3e98: function(arg0, arg1) {
            arg0.dateStyle = __wbindgen_enum_DateTimeStyle[arg1];
        },
        __wbg_set_day_bd62b9b8d7023754: function(arg0, arg1) {
            arg0.day = __wbindgen_enum_DayFormat[arg1];
        },
        __wbg_set_day_period_28db24a8be20e0bb: function(arg0, arg1) {
            arg0.dayPeriod = __wbindgen_enum_DayPeriodFormat[arg1];
        },
        __wbg_set_days_3513965f6d05381e: function(arg0, arg1) {
            arg0.days = __wbindgen_enum_DurationUnitStyle[arg1];
        },
        __wbg_set_days_53248ded92b04ad6: function(arg0, arg1) {
            arg0.days = arg1;
        },
        __wbg_set_days_display_8f833732c9fa7463: function(arg0, arg1) {
            arg0.daysDisplay = __wbindgen_enum_DurationUnitDisplay[arg1];
        },
        __wbg_set_dca99999bba88a9a: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_defaultChecked_0c26327675de942c: function(arg0, arg1) {
            arg0.defaultChecked = arg1 !== 0;
        },
        __wbg_set_defaultValue_ba756b8c3c80e673: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.defaultValue = getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_defaultValue_db8fdb9728a9da22: function(arg0, arg1, arg2) {
            arg0.defaultValue = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_df6c66ce5875f84c: function(arg0, arg1, arg2) {
            arg0.set(getArrayI64FromWasm0(arg1, arg2));
        },
        __wbg_set_dir_0e5920b645c07fed: function(arg0, arg1, arg2) {
            arg0.dir = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_dir_f3e6e9c8695056bb: function(arg0, arg1, arg2) {
            arg0.dir = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_disabled_442b5bf125e44dad: function(arg0, arg1) {
            arg0.disabled = arg1 !== 0;
        },
        __wbg_set_disabled_6410cfc78bcbc1cc: function(arg0, arg1) {
            arg0.disabled = arg1 !== 0;
        },
        __wbg_set_disabled_7e917a62a39f9572: function(arg0, arg1) {
            arg0.disabled = arg1 !== 0;
        },
        __wbg_set_disabled_b9d94dbe27d21a80: function(arg0, arg1) {
            arg0.disabled = arg1 !== 0;
        },
        __wbg_set_download_c8b1176a715864bb: function(arg0, arg1, arg2) {
            arg0.download = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_draggable_efcd7cba1c903421: function(arg0, arg1) {
            arg0.draggable = arg1 !== 0;
        },
        __wbg_set_era_ba13d2695b8a3e82: function(arg0, arg1) {
            arg0.era = __wbindgen_enum_EraFormat[arg1];
        },
        __wbg_set_f189b5e4600a5f27: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_f9ada4d616e318f9: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_fallback_cd3392d1f6987a92: function(arg0, arg1) {
            arg0.fallback = __wbindgen_enum_DisplayNamesFallback[arg1];
        },
        __wbg_set_fc550102ba7d2912: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.set(arg1 >>> 0, arg2);
        }, arguments); },
        __wbg_set_formAction_66f43a7a6609d23a: function(arg0, arg1, arg2) {
            arg0.formAction = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_formEnctype_a28b3ad605b80454: function(arg0, arg1, arg2) {
            arg0.formEnctype = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_formMethod_ab00956cabefc3eb: function(arg0, arg1, arg2) {
            arg0.formMethod = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_formNoValidate_5ec9f031975da93f: function(arg0, arg1) {
            arg0.formNoValidate = arg1 !== 0;
        },
        __wbg_set_formTarget_dc857151cbe82460: function(arg0, arg1, arg2) {
            arg0.formTarget = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_fractional_digits_9625c966e88ab217: function(arg0, arg1) {
            arg0.fractionalDigits = arg1;
        },
        __wbg_set_fractional_second_digits_8f277b48e023cbb6: function(arg0, arg1) {
            arg0.fractionalSecondDigits = arg1;
        },
        __wbg_set_frameBorder_d714ff168b60bd95: function(arg0, arg1, arg2) {
            arg0.frameBorder = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_granularity_1de3cd0e50bdd478: function(arg0, arg1) {
            arg0.granularity = __wbindgen_enum_SegmenterGranularity[arg1];
        },
        __wbg_set_hash_05c629d772e456a4: function(arg0, arg1, arg2) {
            arg0.hash = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_hash_d9e4168071940de2: function(arg0, arg1, arg2) {
            arg0.hash = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_hash_ded05d015cb83b65: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.hash = getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_height_0739170de8653cc4: function(arg0, arg1) {
            arg0.height = arg1 >>> 0;
        },
        __wbg_set_height_6e1d60d97c3f5498: function(arg0, arg1) {
            arg0.height = arg1;
        },
        __wbg_set_height_d9aee4db1b29f776: function(arg0, arg1, arg2) {
            arg0.height = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_height_f58b5894ab85ba7b: function(arg0, arg1) {
            arg0.height = arg1 >>> 0;
        },
        __wbg_set_hidden_708184b8fae3b5b6: function(arg0, arg1) {
            arg0.hidden = arg1 !== 0;
        },
        __wbg_set_host_3f813f341e44590e: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.host = getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_host_7cc7c7d0978a038f: function(arg0, arg1, arg2) {
            arg0.host = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_host_fb8c5edfbef9c0dc: function(arg0, arg1, arg2) {
            arg0.host = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_hostname_577a050fafff67d8: function(arg0, arg1, arg2) {
            arg0.hostname = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_hostname_a9313cd6c828eeb1: function(arg0, arg1, arg2) {
            arg0.hostname = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_hostname_c0c9e24584f55e38: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.hostname = getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_hour12_ac89a36a5b6d660e: function(arg0, arg1) {
            arg0.hour12 = arg1 !== 0;
        },
        __wbg_set_hour_23da84b1d24cb782: function(arg0, arg1) {
            arg0.hour = __wbindgen_enum_NumericFormat[arg1];
        },
        __wbg_set_hour_cycle_cdd25f77c612064f: function(arg0, arg1) {
            arg0.hourCycle = __wbindgen_enum_HourCycle[arg1];
        },
        __wbg_set_hours_6a48b6fbac97cb3b: function(arg0, arg1) {
            arg0.hours = arg1;
        },
        __wbg_set_hours_7cfdf6d4385a8f2d: function(arg0, arg1) {
            arg0.hours = __wbindgen_enum_DurationTimeUnitStyle[arg1];
        },
        __wbg_set_hours_display_bb4451ff2081363d: function(arg0, arg1) {
            arg0.hoursDisplay = __wbindgen_enum_DurationUnitDisplay[arg1];
        },
        __wbg_set_href_028b29649f32e04e: function(arg0, arg1, arg2) {
            arg0.href = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_href_51b19bc5dbf60b48: function(arg0, arg1, arg2) {
            arg0.href = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_href_eee50816257e9d44: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.href = getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_hreflang_5c78624f14d4dcbe: function(arg0, arg1, arg2) {
            arg0.hreflang = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_id_b9d2ee0b28d87959: function(arg0, arg1, arg2) {
            arg0.id = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_ignore_punctuation_e716b8ce86c114fd: function(arg0, arg1) {
            arg0.ignorePunctuation = arg1 !== 0;
        },
        __wbg_set_indeterminate_13ce146d2d6abb8f: function(arg0, arg1) {
            arg0.indeterminate = arg1 !== 0;
        },
        __wbg_set_index_1a78f52a154bc8fd: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = BigInt.asUintN(64, arg2);
        },
        __wbg_set_index_2035f4f34881fdaf: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_index_289a551ad989f695: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_index_2ae12f863484ce58: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_index_34a703f0f99ebe52: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_index_3eee88bdc9246321: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_index_73ced2e1b4a2a37e: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_index_88b4ef962117fc1b: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2 >>> 0;
        },
        __wbg_set_index_8e80bc48545b22b1: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_index_c69336ea758c0507: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_index_cce046e0b3771dc8: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_index_from_f32_18159b4b5d6c93ed: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_inert_3c77ce43ca4ec717: function(arg0, arg1) {
            arg0.inert = arg1 !== 0;
        },
        __wbg_set_innerHTML_6bcbbce0a3626998: function(arg0, arg1, arg2) {
            arg0.innerHTML = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_innerHeight_f34f8fb7f8b6045b: function() { return handleError(function (arg0, arg1) {
            arg0.innerHeight = arg1;
        }, arguments); },
        __wbg_set_innerText_2126c17ae88dc653: function(arg0, arg1, arg2) {
            arg0.innerText = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_innerWidth_97f1ecce7a937e2b: function() { return handleError(function (arg0, arg1) {
            arg0.innerWidth = arg1;
        }, arguments); },
        __wbg_set_inputMode_e4b5bd5e15f96908: function(arg0, arg1, arg2) {
            arg0.inputMode = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_lang_98db188c9db5210e: function(arg0, arg1, arg2) {
            arg0.lang = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_language_display_6a9cab3f438714f8: function(arg0, arg1) {
            arg0.languageDisplay = __wbindgen_enum_DisplayNamesLanguageDisplay[arg1];
        },
        __wbg_set_last_index_73f584e6d942d3de: function(arg0, arg1) {
            arg0.lastIndex = arg1 >>> 0;
        },
        __wbg_set_locale_matcher_0ca58d8362cb9870: function(arg0, arg1) {
            arg0.localeMatcher = __wbindgen_enum_LocaleMatcher[arg1];
        },
        __wbg_set_locale_matcher_340315129956aa71: function(arg0, arg1) {
            arg0.localeMatcher = __wbindgen_enum_LocaleMatcher[arg1];
        },
        __wbg_set_locale_matcher_46595e63f3ae6f89: function(arg0, arg1) {
            arg0.localeMatcher = __wbindgen_enum_LocaleMatcher[arg1];
        },
        __wbg_set_locale_matcher_4cd9264b55a93721: function(arg0, arg1) {
            arg0.localeMatcher = __wbindgen_enum_LocaleMatcher[arg1];
        },
        __wbg_set_locale_matcher_6eff52fbfe22cb5e: function(arg0, arg1) {
            arg0.localeMatcher = __wbindgen_enum_LocaleMatcher[arg1];
        },
        __wbg_set_locale_matcher_706feec405c756eb: function(arg0, arg1) {
            arg0.localeMatcher = __wbindgen_enum_LocaleMatcher[arg1];
        },
        __wbg_set_locale_matcher_7299b5e70b58d092: function(arg0, arg1) {
            arg0.localeMatcher = __wbindgen_enum_LocaleMatcher[arg1];
        },
        __wbg_set_locale_matcher_732bdc7162815720: function(arg0, arg1) {
            arg0.localeMatcher = __wbindgen_enum_LocaleMatcher[arg1];
        },
        __wbg_set_locale_matcher_85eb2a26ba2fb007: function(arg0, arg1) {
            arg0.localeMatcher = __wbindgen_enum_LocaleMatcher[arg1];
        },
        __wbg_set_locale_matcher_c8af450b5f9b212c: function(arg0, arg1) {
            arg0.localeMatcher = __wbindgen_enum_LocaleMatcher[arg1];
        },
        __wbg_set_longDesc_16e27da609e7a2c9: function(arg0, arg1, arg2) {
            arg0.longDesc = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_marginHeight_29584486dc2f3881: function(arg0, arg1, arg2) {
            arg0.marginHeight = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_marginWidth_3bd8b4c9bc11fe38: function(arg0, arg1, arg2) {
            arg0.marginWidth = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_maxByteLength_a7ef641de9279a61: function(arg0, arg1) {
            arg0.maxByteLength = arg1 >>> 0;
        },
        __wbg_set_maxLength_8e7a20c4568baa45: function(arg0, arg1) {
            arg0.maxLength = arg1;
        },
        __wbg_set_maxLength_afc3e0b08fc97d19: function(arg0, arg1) {
            arg0.maxLength = arg1;
        },
        __wbg_set_max_47993604f9b5f5ce: function(arg0, arg1, arg2) {
            arg0.max = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_maximum_fraction_digits_c972ac7fc5bf9d7d: function(arg0, arg1) {
            arg0.maximumFractionDigits = arg1;
        },
        __wbg_set_maximum_fraction_digits_e156cada357f7e6e: function(arg0, arg1) {
            arg0.maximumFractionDigits = arg1;
        },
        __wbg_set_maximum_significant_digits_3d1c12a56fc01940: function(arg0, arg1) {
            arg0.maximumSignificantDigits = arg1;
        },
        __wbg_set_maximum_significant_digits_c5e4487fc97597fa: function(arg0, arg1) {
            arg0.maximumSignificantDigits = arg1;
        },
        __wbg_set_media_24dc0e3c8bb1b3cf: function(arg0, arg1, arg2) {
            arg0.media = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_message_2d8e9faaa4550a14: function(arg0, arg1, arg2) {
            arg0.message = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_microseconds_21794c09f247b3d2: function(arg0, arg1) {
            arg0.microseconds = arg1;
        },
        __wbg_set_microseconds_display_a7b34805b851c4eb: function(arg0, arg1) {
            arg0.microsecondsDisplay = __wbindgen_enum_DurationUnitDisplay[arg1];
        },
        __wbg_set_microseconds_ecd835b880e9bbcd: function(arg0, arg1) {
            arg0.microseconds = __wbindgen_enum_DurationUnitStyle[arg1];
        },
        __wbg_set_milliseconds_9cc577c5f6369e2f: function(arg0, arg1) {
            arg0.milliseconds = arg1;
        },
        __wbg_set_milliseconds_display_d410be8ccfc66fd6: function(arg0, arg1) {
            arg0.millisecondsDisplay = __wbindgen_enum_DurationUnitDisplay[arg1];
        },
        __wbg_set_milliseconds_f4a25ed7cec35ca7: function(arg0, arg1) {
            arg0.milliseconds = __wbindgen_enum_DurationUnitStyle[arg1];
        },
        __wbg_set_minLength_0dfd3d2504e40389: function(arg0, arg1) {
            arg0.minLength = arg1;
        },
        __wbg_set_minLength_540f0a4d7808458e: function(arg0, arg1) {
            arg0.minLength = arg1;
        },
        __wbg_set_min_24516c6e14f25498: function(arg0, arg1, arg2) {
            arg0.min = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_minimum_fraction_digits_70bbaed2667336c1: function(arg0, arg1) {
            arg0.minimumFractionDigits = arg1;
        },
        __wbg_set_minimum_fraction_digits_9c5bfd25544c76d3: function(arg0, arg1) {
            arg0.minimumFractionDigits = arg1;
        },
        __wbg_set_minimum_integer_digits_53d157367bb3995d: function(arg0, arg1) {
            arg0.minimumIntegerDigits = arg1;
        },
        __wbg_set_minimum_integer_digits_f7edeb987c490b88: function(arg0, arg1) {
            arg0.minimumIntegerDigits = arg1;
        },
        __wbg_set_minimum_significant_digits_42fb2de494c7d9e8: function(arg0, arg1) {
            arg0.minimumSignificantDigits = arg1;
        },
        __wbg_set_minimum_significant_digits_51f87384f0f3c585: function(arg0, arg1) {
            arg0.minimumSignificantDigits = arg1;
        },
        __wbg_set_minute_4c039e758f9ed0a8: function(arg0, arg1) {
            arg0.minute = __wbindgen_enum_NumericFormat[arg1];
        },
        __wbg_set_minutes_27711f766a24e0c1: function(arg0, arg1) {
            arg0.minutes = arg1;
        },
        __wbg_set_minutes_display_4cc942e8b1983849: function(arg0, arg1) {
            arg0.minutesDisplay = __wbindgen_enum_DurationUnitDisplay[arg1];
        },
        __wbg_set_minutes_f5e6893654347252: function(arg0, arg1) {
            arg0.minutes = __wbindgen_enum_DurationTimeUnitStyle[arg1];
        },
        __wbg_set_month_053ac1d39fbd78f6: function(arg0, arg1) {
            arg0.month = __wbindgen_enum_MonthFormat[arg1];
        },
        __wbg_set_months_1d31d8fdd27e8c6e: function(arg0, arg1) {
            arg0.months = __wbindgen_enum_DurationUnitStyle[arg1];
        },
        __wbg_set_months_cb1d97d06d880996: function(arg0, arg1) {
            arg0.months = arg1;
        },
        __wbg_set_months_display_0d9019d8897c043c: function(arg0, arg1) {
            arg0.monthsDisplay = __wbindgen_enum_DurationUnitDisplay[arg1];
        },
        __wbg_set_multiple_c08104d2a9537f5e: function(arg0, arg1) {
            arg0.multiple = arg1 !== 0;
        },
        __wbg_set_name_04ebefb4f702af61: function(arg0, arg1, arg2) {
            arg0.name = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_name_201fd981ad97e2c0: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.name = getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_name_3c3fc49c8f747e56: function(arg0, arg1, arg2) {
            arg0.name = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_name_932b5fd6d5773729: function(arg0, arg1, arg2) {
            arg0.name = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_name_a272aabe789947a9: function(arg0, arg1, arg2) {
            arg0.name = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_name_a2e492d3289b8fb2: function(arg0, arg1, arg2) {
            arg0.name = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_nanoseconds_712ff833101b7d60: function(arg0, arg1) {
            arg0.nanoseconds = arg1;
        },
        __wbg_set_nanoseconds_7e2b1ae1618c0f55: function(arg0, arg1) {
            arg0.nanoseconds = __wbindgen_enum_DurationUnitStyle[arg1];
        },
        __wbg_set_nanoseconds_display_96f2430d047a4131: function(arg0, arg1) {
            arg0.nanosecondsDisplay = __wbindgen_enum_DurationUnitDisplay[arg1];
        },
        __wbg_set_nodeValue_d6d0279af930c379: function(arg0, arg1, arg2) {
            arg0.nodeValue = arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_nonce_62c330d61cae5504: function(arg0, arg1, arg2) {
            arg0.nonce = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_notation_69dc8719f8b4a802: function(arg0, arg1) {
            arg0.notation = __wbindgen_enum_NumberFormatNotation[arg1];
        },
        __wbg_set_numbering_system_0fd36d3ed5405890: function(arg0, arg1, arg2) {
            arg0.numberingSystem = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_numbering_system_f604eb3e75e3e973: function(arg0, arg1, arg2) {
            arg0.numberingSystem = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_numeric_bd79d22a8380c249: function(arg0, arg1) {
            arg0.numeric = arg1 !== 0;
        },
        __wbg_set_numeric_d1e791de1b298dc4: function(arg0, arg1) {
            arg0.numeric = __wbindgen_enum_RelativeTimeFormatNumeric[arg1];
        },
        __wbg_set_onabort_4acc3fbbd933e43f: function(arg0, arg1) {
            arg0.onabort = arg1;
        },
        __wbg_set_onabort_879b6e328a958ad6: function(arg0, arg1) {
            arg0.onabort = arg1;
        },
        __wbg_set_onabort_db3ab9aedabfcecd: function(arg0, arg1) {
            arg0.onabort = arg1;
        },
        __wbg_set_onafterprint_33f4db97ca097868: function(arg0, arg1) {
            arg0.onafterprint = arg1;
        },
        __wbg_set_onafterscriptexecute_456336e37ba33922: function(arg0, arg1) {
            arg0.onafterscriptexecute = arg1;
        },
        __wbg_set_onanimationcancel_2abad71e341ac28a: function(arg0, arg1) {
            arg0.onanimationcancel = arg1;
        },
        __wbg_set_onanimationcancel_6878daadb7816dee: function(arg0, arg1) {
            arg0.onanimationcancel = arg1;
        },
        __wbg_set_onanimationcancel_8096b2e5e4e47949: function(arg0, arg1) {
            arg0.onanimationcancel = arg1;
        },
        __wbg_set_onanimationend_2b238225ca3a6619: function(arg0, arg1) {
            arg0.onanimationend = arg1;
        },
        __wbg_set_onanimationend_756179cdb81b9d51: function(arg0, arg1) {
            arg0.onanimationend = arg1;
        },
        __wbg_set_onanimationend_da191fab9361e2ae: function(arg0, arg1) {
            arg0.onanimationend = arg1;
        },
        __wbg_set_onanimationiteration_2b0d99be435adde6: function(arg0, arg1) {
            arg0.onanimationiteration = arg1;
        },
        __wbg_set_onanimationiteration_61caa34e23199fd0: function(arg0, arg1) {
            arg0.onanimationiteration = arg1;
        },
        __wbg_set_onanimationiteration_dadb685762afbd45: function(arg0, arg1) {
            arg0.onanimationiteration = arg1;
        },
        __wbg_set_onanimationstart_16dba59f78220fcb: function(arg0, arg1) {
            arg0.onanimationstart = arg1;
        },
        __wbg_set_onanimationstart_6757247207d7604a: function(arg0, arg1) {
            arg0.onanimationstart = arg1;
        },
        __wbg_set_onanimationstart_80069ba9ba4bcbda: function(arg0, arg1) {
            arg0.onanimationstart = arg1;
        },
        __wbg_set_onappinstalled_b58d6c416b196eeb: function(arg0, arg1) {
            arg0.onappinstalled = arg1;
        },
        __wbg_set_onauxclick_cb90314cca3056fe: function(arg0, arg1) {
            arg0.onauxclick = arg1;
        },
        __wbg_set_onauxclick_e3dac1737db77056: function(arg0, arg1) {
            arg0.onauxclick = arg1;
        },
        __wbg_set_onauxclick_ed3326605c52b135: function(arg0, arg1) {
            arg0.onauxclick = arg1;
        },
        __wbg_set_onbeforeinput_682f10b9b8ecd053: function(arg0, arg1) {
            arg0.onbeforeinput = arg1;
        },
        __wbg_set_onbeforeinput_8a4c7e7620df6160: function(arg0, arg1) {
            arg0.onbeforeinput = arg1;
        },
        __wbg_set_onbeforeinput_d78133664df55654: function(arg0, arg1) {
            arg0.onbeforeinput = arg1;
        },
        __wbg_set_onbeforeprint_b220202fe62df4b6: function(arg0, arg1) {
            arg0.onbeforeprint = arg1;
        },
        __wbg_set_onbeforescriptexecute_82763e6437729f00: function(arg0, arg1) {
            arg0.onbeforescriptexecute = arg1;
        },
        __wbg_set_onbeforetoggle_27e31d40615afab9: function(arg0, arg1) {
            arg0.onbeforetoggle = arg1;
        },
        __wbg_set_onbeforetoggle_5692f21fe9a3cced: function(arg0, arg1) {
            arg0.onbeforetoggle = arg1;
        },
        __wbg_set_onbeforetoggle_d9c033d2c4a433d7: function(arg0, arg1) {
            arg0.onbeforetoggle = arg1;
        },
        __wbg_set_onbeforeunload_a807349afe398752: function(arg0, arg1) {
            arg0.onbeforeunload = arg1;
        },
        __wbg_set_onblur_126c41be33ad5a93: function(arg0, arg1) {
            arg0.onblur = arg1;
        },
        __wbg_set_onblur_35f248587f0e6b5d: function(arg0, arg1) {
            arg0.onblur = arg1;
        },
        __wbg_set_onblur_50d03f53cd0b72c2: function(arg0, arg1) {
            arg0.onblur = arg1;
        },
        __wbg_set_oncancel_22df455f1f4b8aeb: function(arg0, arg1) {
            arg0.oncancel = arg1;
        },
        __wbg_set_oncancel_57fda673cfea03d6: function(arg0, arg1) {
            arg0.oncancel = arg1;
        },
        __wbg_set_oncancel_aa631d2749b6454d: function(arg0, arg1) {
            arg0.oncancel = arg1;
        },
        __wbg_set_oncanplay_20d24e63f3ccd1e2: function(arg0, arg1) {
            arg0.oncanplay = arg1;
        },
        __wbg_set_oncanplay_3e0995a57b5c7c86: function(arg0, arg1) {
            arg0.oncanplay = arg1;
        },
        __wbg_set_oncanplay_df6079741faccecd: function(arg0, arg1) {
            arg0.oncanplay = arg1;
        },
        __wbg_set_oncanplaythrough_385422e443baac39: function(arg0, arg1) {
            arg0.oncanplaythrough = arg1;
        },
        __wbg_set_oncanplaythrough_8b1721ef00416bc0: function(arg0, arg1) {
            arg0.oncanplaythrough = arg1;
        },
        __wbg_set_oncanplaythrough_c89d7db4ed68b0b6: function(arg0, arg1) {
            arg0.oncanplaythrough = arg1;
        },
        __wbg_set_onchange_5a81ed420cd31e59: function(arg0, arg1) {
            arg0.onchange = arg1;
        },
        __wbg_set_onchange_5cc24e0710891c57: function(arg0, arg1) {
            arg0.onchange = arg1;
        },
        __wbg_set_onchange_ad79de76d7347751: function(arg0, arg1) {
            arg0.onchange = arg1;
        },
        __wbg_set_onchange_b3e8750f3ff32af9: function(arg0, arg1) {
            arg0.onchange = arg1;
        },
        __wbg_set_onclick_4d96c6b7b9e25273: function(arg0, arg1) {
            arg0.onclick = arg1;
        },
        __wbg_set_onclick_7812608501bf59da: function(arg0, arg1) {
            arg0.onclick = arg1;
        },
        __wbg_set_onclick_9cd900c17dd4c05d: function(arg0, arg1) {
            arg0.onclick = arg1;
        },
        __wbg_set_onclose_13787fb31ae8aefd: function(arg0, arg1) {
            arg0.onclose = arg1;
        },
        __wbg_set_onclose_4ff51ca38015e2ff: function(arg0, arg1) {
            arg0.onclose = arg1;
        },
        __wbg_set_onclose_a17a6924d1ad86c4: function(arg0, arg1) {
            arg0.onclose = arg1;
        },
        __wbg_set_onclose_a7211b02c160885e: function(arg0, arg1) {
            arg0.onclose = arg1;
        },
        __wbg_set_oncontextmenu_0f7257edb714a1f9: function(arg0, arg1) {
            arg0.oncontextmenu = arg1;
        },
        __wbg_set_oncontextmenu_598e0fc9a0a4bd11: function(arg0, arg1) {
            arg0.oncontextmenu = arg1;
        },
        __wbg_set_oncontextmenu_7443fe7213ced6c9: function(arg0, arg1) {
            arg0.oncontextmenu = arg1;
        },
        __wbg_set_oncopy_0d18f6ea85d41f05: function(arg0, arg1) {
            arg0.oncopy = arg1;
        },
        __wbg_set_oncopy_3e5ebe923389b632: function(arg0, arg1) {
            arg0.oncopy = arg1;
        },
        __wbg_set_oncut_d1e79f259368bbc9: function(arg0, arg1) {
            arg0.oncut = arg1;
        },
        __wbg_set_oncut_d6da5fabe73eb4c6: function(arg0, arg1) {
            arg0.oncut = arg1;
        },
        __wbg_set_ondblclick_0b913913fe62ed39: function(arg0, arg1) {
            arg0.ondblclick = arg1;
        },
        __wbg_set_ondblclick_2897b87b39b035eb: function(arg0, arg1) {
            arg0.ondblclick = arg1;
        },
        __wbg_set_ondblclick_7084a3f905408175: function(arg0, arg1) {
            arg0.ondblclick = arg1;
        },
        __wbg_set_ondrag_5c752c86160e0add: function(arg0, arg1) {
            arg0.ondrag = arg1;
        },
        __wbg_set_ondrag_5e276f636a282494: function(arg0, arg1) {
            arg0.ondrag = arg1;
        },
        __wbg_set_ondrag_9e759e7ff411c5eb: function(arg0, arg1) {
            arg0.ondrag = arg1;
        },
        __wbg_set_ondragend_12ce2c0cfbbfe67d: function(arg0, arg1) {
            arg0.ondragend = arg1;
        },
        __wbg_set_ondragend_31ca2c67efa76e30: function(arg0, arg1) {
            arg0.ondragend = arg1;
        },
        __wbg_set_ondragend_f9f58f271ac7adfb: function(arg0, arg1) {
            arg0.ondragend = arg1;
        },
        __wbg_set_ondragenter_1fa7832ed455a63d: function(arg0, arg1) {
            arg0.ondragenter = arg1;
        },
        __wbg_set_ondragenter_4a6968486cda74e3: function(arg0, arg1) {
            arg0.ondragenter = arg1;
        },
        __wbg_set_ondragenter_e602e93ce6580730: function(arg0, arg1) {
            arg0.ondragenter = arg1;
        },
        __wbg_set_ondragexit_8c948208babd4251: function(arg0, arg1) {
            arg0.ondragexit = arg1;
        },
        __wbg_set_ondragexit_9a4f4d03326f6b6b: function(arg0, arg1) {
            arg0.ondragexit = arg1;
        },
        __wbg_set_ondragexit_fd056d02b326c0af: function(arg0, arg1) {
            arg0.ondragexit = arg1;
        },
        __wbg_set_ondragleave_1f47aa848fbbd5cb: function(arg0, arg1) {
            arg0.ondragleave = arg1;
        },
        __wbg_set_ondragleave_788aa4a520416663: function(arg0, arg1) {
            arg0.ondragleave = arg1;
        },
        __wbg_set_ondragleave_a1693271ad4c8309: function(arg0, arg1) {
            arg0.ondragleave = arg1;
        },
        __wbg_set_ondragover_4ad3551b0d263990: function(arg0, arg1) {
            arg0.ondragover = arg1;
        },
        __wbg_set_ondragover_72c577e986e7d4f8: function(arg0, arg1) {
            arg0.ondragover = arg1;
        },
        __wbg_set_ondragover_9ff9bf2d3d586543: function(arg0, arg1) {
            arg0.ondragover = arg1;
        },
        __wbg_set_ondragstart_348d233b7644f7bd: function(arg0, arg1) {
            arg0.ondragstart = arg1;
        },
        __wbg_set_ondragstart_a66ff43b7ffcef0e: function(arg0, arg1) {
            arg0.ondragstart = arg1;
        },
        __wbg_set_ondragstart_e8ca47adfe506e45: function(arg0, arg1) {
            arg0.ondragstart = arg1;
        },
        __wbg_set_ondrop_41eb92f0ca022254: function(arg0, arg1) {
            arg0.ondrop = arg1;
        },
        __wbg_set_ondrop_e92cb82efdebda2c: function(arg0, arg1) {
            arg0.ondrop = arg1;
        },
        __wbg_set_ondrop_f7cd3eec29a4b0d1: function(arg0, arg1) {
            arg0.ondrop = arg1;
        },
        __wbg_set_ondurationchange_250a92a3652795d9: function(arg0, arg1) {
            arg0.ondurationchange = arg1;
        },
        __wbg_set_ondurationchange_e561bd12ec903f6a: function(arg0, arg1) {
            arg0.ondurationchange = arg1;
        },
        __wbg_set_ondurationchange_fa7c9691a674c285: function(arg0, arg1) {
            arg0.ondurationchange = arg1;
        },
        __wbg_set_onemptied_42068ac564ea9f6e: function(arg0, arg1) {
            arg0.onemptied = arg1;
        },
        __wbg_set_onemptied_aaccd4a44b1db6d7: function(arg0, arg1) {
            arg0.onemptied = arg1;
        },
        __wbg_set_onemptied_f018f0d436659311: function(arg0, arg1) {
            arg0.onemptied = arg1;
        },
        __wbg_set_onended_2bc8f784c1f7822c: function(arg0, arg1) {
            arg0.onended = arg1;
        },
        __wbg_set_onended_7a83cc73c5a2d1a5: function(arg0, arg1) {
            arg0.onended = arg1;
        },
        __wbg_set_onended_7d5bcb19f148e878: function(arg0, arg1) {
            arg0.onended = arg1;
        },
        __wbg_set_onerror_193792aaaf8397b3: function(arg0, arg1) {
            arg0.onerror = arg1;
        },
        __wbg_set_onerror_36dab24632e45990: function(arg0, arg1) {
            arg0.onerror = arg1;
        },
        __wbg_set_onerror_5a45265839edf1b1: function(arg0, arg1) {
            arg0.onerror = arg1;
        },
        __wbg_set_onerror_669dc9c209302a13: function(arg0, arg1) {
            arg0.onerror = arg1;
        },
        __wbg_set_onfocus_0a431a7bf2e8801f: function(arg0, arg1) {
            arg0.onfocus = arg1;
        },
        __wbg_set_onfocus_24375bc7c5f5923f: function(arg0, arg1) {
            arg0.onfocus = arg1;
        },
        __wbg_set_onfocus_82014c6819945943: function(arg0, arg1) {
            arg0.onfocus = arg1;
        },
        __wbg_set_onfullscreenchange_c4b664677ccfa71d: function(arg0, arg1) {
            arg0.onfullscreenchange = arg1;
        },
        __wbg_set_onfullscreenerror_08a049c7aa957ed6: function(arg0, arg1) {
            arg0.onfullscreenerror = arg1;
        },
        __wbg_set_ongotpointercapture_7905b691e721f647: function(arg0, arg1) {
            arg0.ongotpointercapture = arg1;
        },
        __wbg_set_ongotpointercapture_c1d113169817b0f4: function(arg0, arg1) {
            arg0.ongotpointercapture = arg1;
        },
        __wbg_set_ongotpointercapture_ed327893597d9fec: function(arg0, arg1) {
            arg0.ongotpointercapture = arg1;
        },
        __wbg_set_onhashchange_a15cf47739c03ac6: function(arg0, arg1) {
            arg0.onhashchange = arg1;
        },
        __wbg_set_oninput_08e8223234720083: function(arg0, arg1) {
            arg0.oninput = arg1;
        },
        __wbg_set_oninput_343be4edddddaa16: function(arg0, arg1) {
            arg0.oninput = arg1;
        },
        __wbg_set_oninput_b6ce731d7b89175e: function(arg0, arg1) {
            arg0.oninput = arg1;
        },
        __wbg_set_oninvalid_528720f3e879eb79: function(arg0, arg1) {
            arg0.oninvalid = arg1;
        },
        __wbg_set_oninvalid_5bf3874aa8125dfb: function(arg0, arg1) {
            arg0.oninvalid = arg1;
        },
        __wbg_set_oninvalid_ddf7fd72b6325023: function(arg0, arg1) {
            arg0.oninvalid = arg1;
        },
        __wbg_set_onkeydown_100e7bea55746347: function(arg0, arg1) {
            arg0.onkeydown = arg1;
        },
        __wbg_set_onkeydown_fa5d44347946fd1a: function(arg0, arg1) {
            arg0.onkeydown = arg1;
        },
        __wbg_set_onkeydown_fb9e04edbf3a4fa0: function(arg0, arg1) {
            arg0.onkeydown = arg1;
        },
        __wbg_set_onkeypress_76404f4f7f53ea4f: function(arg0, arg1) {
            arg0.onkeypress = arg1;
        },
        __wbg_set_onkeypress_b334767b0e0777bc: function(arg0, arg1) {
            arg0.onkeypress = arg1;
        },
        __wbg_set_onkeypress_ffb99087e1e36f06: function(arg0, arg1) {
            arg0.onkeypress = arg1;
        },
        __wbg_set_onkeyup_4c230d836fa678da: function(arg0, arg1) {
            arg0.onkeyup = arg1;
        },
        __wbg_set_onkeyup_eb4b585de00e76ef: function(arg0, arg1) {
            arg0.onkeyup = arg1;
        },
        __wbg_set_onkeyup_efe4ca5c40b0097c: function(arg0, arg1) {
            arg0.onkeyup = arg1;
        },
        __wbg_set_onlanguagechange_188295cbc113d3a8: function(arg0, arg1) {
            arg0.onlanguagechange = arg1;
        },
        __wbg_set_onload_2340291f7b993b23: function(arg0, arg1) {
            arg0.onload = arg1;
        },
        __wbg_set_onload_409668aa7a13cfbc: function(arg0, arg1) {
            arg0.onload = arg1;
        },
        __wbg_set_onload_5fe21ae9b2e1f50c: function(arg0, arg1) {
            arg0.onload = arg1;
        },
        __wbg_set_onloadeddata_57909450bb515eb2: function(arg0, arg1) {
            arg0.onloadeddata = arg1;
        },
        __wbg_set_onloadeddata_d756de61a04e4b87: function(arg0, arg1) {
            arg0.onloadeddata = arg1;
        },
        __wbg_set_onloadeddata_d9054fd128cbe157: function(arg0, arg1) {
            arg0.onloadeddata = arg1;
        },
        __wbg_set_onloadedmetadata_54c54f5a3e030b04: function(arg0, arg1) {
            arg0.onloadedmetadata = arg1;
        },
        __wbg_set_onloadedmetadata_a3eef39c6e6ae746: function(arg0, arg1) {
            arg0.onloadedmetadata = arg1;
        },
        __wbg_set_onloadedmetadata_ea5b9d36ae865bd4: function(arg0, arg1) {
            arg0.onloadedmetadata = arg1;
        },
        __wbg_set_onloadend_8f3a107495aa8ac9: function(arg0, arg1) {
            arg0.onloadend = arg1;
        },
        __wbg_set_onloadend_98330daf4233d128: function(arg0, arg1) {
            arg0.onloadend = arg1;
        },
        __wbg_set_onloadend_c59ccbda0d53367e: function(arg0, arg1) {
            arg0.onloadend = arg1;
        },
        __wbg_set_onloadstart_07eeae1d5c7092ae: function(arg0, arg1) {
            arg0.onloadstart = arg1;
        },
        __wbg_set_onloadstart_45265ccafd41b0bb: function(arg0, arg1) {
            arg0.onloadstart = arg1;
        },
        __wbg_set_onloadstart_54c70764de53e690: function(arg0, arg1) {
            arg0.onloadstart = arg1;
        },
        __wbg_set_onlostpointercapture_01a01807038bea6e: function(arg0, arg1) {
            arg0.onlostpointercapture = arg1;
        },
        __wbg_set_onlostpointercapture_55e08cdff6939fcc: function(arg0, arg1) {
            arg0.onlostpointercapture = arg1;
        },
        __wbg_set_onlostpointercapture_57ec44920cb841c3: function(arg0, arg1) {
            arg0.onlostpointercapture = arg1;
        },
        __wbg_set_onmessage_0c8dcd7703b6e134: function(arg0, arg1) {
            arg0.onmessage = arg1;
        },
        __wbg_set_onmessage_9c6b4cb14e244b7f: function(arg0, arg1) {
            arg0.onmessage = arg1;
        },
        __wbg_set_onmessageerror_391cc7d246e63077: function(arg0, arg1) {
            arg0.onmessageerror = arg1;
        },
        __wbg_set_onmousedown_0b46c355a1fc9994: function(arg0, arg1) {
            arg0.onmousedown = arg1;
        },
        __wbg_set_onmousedown_6815c4b6368bfac3: function(arg0, arg1) {
            arg0.onmousedown = arg1;
        },
        __wbg_set_onmousedown_95e60e1dc94574b7: function(arg0, arg1) {
            arg0.onmousedown = arg1;
        },
        __wbg_set_onmouseenter_18652b9fd39954ce: function(arg0, arg1) {
            arg0.onmouseenter = arg1;
        },
        __wbg_set_onmouseenter_1e7a5dfc9d15441d: function(arg0, arg1) {
            arg0.onmouseenter = arg1;
        },
        __wbg_set_onmouseenter_31e1eb71860f09d1: function(arg0, arg1) {
            arg0.onmouseenter = arg1;
        },
        __wbg_set_onmouseleave_24b64828ac1401b7: function(arg0, arg1) {
            arg0.onmouseleave = arg1;
        },
        __wbg_set_onmouseleave_b3a3fb80c4d812aa: function(arg0, arg1) {
            arg0.onmouseleave = arg1;
        },
        __wbg_set_onmouseleave_e0b2de019caf38a6: function(arg0, arg1) {
            arg0.onmouseleave = arg1;
        },
        __wbg_set_onmousemove_03444772d876ccf8: function(arg0, arg1) {
            arg0.onmousemove = arg1;
        },
        __wbg_set_onmousemove_c4383d3d1d9184cf: function(arg0, arg1) {
            arg0.onmousemove = arg1;
        },
        __wbg_set_onmousemove_c95191497e8e50eb: function(arg0, arg1) {
            arg0.onmousemove = arg1;
        },
        __wbg_set_onmouseout_638d6ea1b3f95b39: function(arg0, arg1) {
            arg0.onmouseout = arg1;
        },
        __wbg_set_onmouseout_a29d8ee2d4e70a38: function(arg0, arg1) {
            arg0.onmouseout = arg1;
        },
        __wbg_set_onmouseout_b3db09b02ceabd88: function(arg0, arg1) {
            arg0.onmouseout = arg1;
        },
        __wbg_set_onmouseover_1070aa4f9e2bc6aa: function(arg0, arg1) {
            arg0.onmouseover = arg1;
        },
        __wbg_set_onmouseover_c30e04a9f3a9d2d0: function(arg0, arg1) {
            arg0.onmouseover = arg1;
        },
        __wbg_set_onmouseover_fce3ad30e7553479: function(arg0, arg1) {
            arg0.onmouseover = arg1;
        },
        __wbg_set_onmouseup_1c6ec38ba2605043: function(arg0, arg1) {
            arg0.onmouseup = arg1;
        },
        __wbg_set_onmouseup_269d3b08afe6fb33: function(arg0, arg1) {
            arg0.onmouseup = arg1;
        },
        __wbg_set_onmouseup_fb3dde2a29c65aee: function(arg0, arg1) {
            arg0.onmouseup = arg1;
        },
        __wbg_set_onoffline_8fc645c146d9dc99: function(arg0, arg1) {
            arg0.onoffline = arg1;
        },
        __wbg_set_ononline_e19effe42b2f7f44: function(arg0, arg1) {
            arg0.ononline = arg1;
        },
        __wbg_set_onopen_db452f4233e99d7d: function(arg0, arg1) {
            arg0.onopen = arg1;
        },
        __wbg_set_onorientationchange_c3a1fbf73967a3a7: function(arg0, arg1) {
            arg0.onorientationchange = arg1;
        },
        __wbg_set_onpagehide_b13bbec9f215bd0b: function(arg0, arg1) {
            arg0.onpagehide = arg1;
        },
        __wbg_set_onpageshow_c3579c814437f24e: function(arg0, arg1) {
            arg0.onpageshow = arg1;
        },
        __wbg_set_onpaste_6cba0658b2a17832: function(arg0, arg1) {
            arg0.onpaste = arg1;
        },
        __wbg_set_onpaste_bdbd1c7281499dd7: function(arg0, arg1) {
            arg0.onpaste = arg1;
        },
        __wbg_set_onpause_100af47291461d78: function(arg0, arg1) {
            arg0.onpause = arg1;
        },
        __wbg_set_onpause_c3390ac7f7b5cff9: function(arg0, arg1) {
            arg0.onpause = arg1;
        },
        __wbg_set_onpause_e415020d4f8ebad5: function(arg0, arg1) {
            arg0.onpause = arg1;
        },
        __wbg_set_onplay_447a39bac6546fc5: function(arg0, arg1) {
            arg0.onplay = arg1;
        },
        __wbg_set_onplay_822bfa4c42a6bda8: function(arg0, arg1) {
            arg0.onplay = arg1;
        },
        __wbg_set_onplay_a0f342c65b6ef1cb: function(arg0, arg1) {
            arg0.onplay = arg1;
        },
        __wbg_set_onplaying_2e2aef119efebe93: function(arg0, arg1) {
            arg0.onplaying = arg1;
        },
        __wbg_set_onplaying_9ab878a8efdc683e: function(arg0, arg1) {
            arg0.onplaying = arg1;
        },
        __wbg_set_onplaying_fd9c11da1d6ab4c7: function(arg0, arg1) {
            arg0.onplaying = arg1;
        },
        __wbg_set_onpointercancel_2fb1bf02f32da85b: function(arg0, arg1) {
            arg0.onpointercancel = arg1;
        },
        __wbg_set_onpointercancel_da78ad5f4c507792: function(arg0, arg1) {
            arg0.onpointercancel = arg1;
        },
        __wbg_set_onpointercancel_fd4e65e5b3ea9bee: function(arg0, arg1) {
            arg0.onpointercancel = arg1;
        },
        __wbg_set_onpointerdown_25f420e30cba7f45: function(arg0, arg1) {
            arg0.onpointerdown = arg1;
        },
        __wbg_set_onpointerdown_b8e353db7de17ea9: function(arg0, arg1) {
            arg0.onpointerdown = arg1;
        },
        __wbg_set_onpointerdown_ec985919a4724803: function(arg0, arg1) {
            arg0.onpointerdown = arg1;
        },
        __wbg_set_onpointerenter_18533c9f7e73fce7: function(arg0, arg1) {
            arg0.onpointerenter = arg1;
        },
        __wbg_set_onpointerenter_4d62475deafd1aeb: function(arg0, arg1) {
            arg0.onpointerenter = arg1;
        },
        __wbg_set_onpointerenter_e36d2c1ebbfa8c7c: function(arg0, arg1) {
            arg0.onpointerenter = arg1;
        },
        __wbg_set_onpointerleave_3dd285632162aa10: function(arg0, arg1) {
            arg0.onpointerleave = arg1;
        },
        __wbg_set_onpointerleave_91845ee253761e0a: function(arg0, arg1) {
            arg0.onpointerleave = arg1;
        },
        __wbg_set_onpointerleave_976a6890d993d668: function(arg0, arg1) {
            arg0.onpointerleave = arg1;
        },
        __wbg_set_onpointerlockchange_c84e336105578261: function(arg0, arg1) {
            arg0.onpointerlockchange = arg1;
        },
        __wbg_set_onpointerlockerror_090462a18840cd2c: function(arg0, arg1) {
            arg0.onpointerlockerror = arg1;
        },
        __wbg_set_onpointermove_4865e2683df413a0: function(arg0, arg1) {
            arg0.onpointermove = arg1;
        },
        __wbg_set_onpointermove_bb9f5cc492083585: function(arg0, arg1) {
            arg0.onpointermove = arg1;
        },
        __wbg_set_onpointermove_fef4e2597049ffe6: function(arg0, arg1) {
            arg0.onpointermove = arg1;
        },
        __wbg_set_onpointerout_15e482abfc7adb56: function(arg0, arg1) {
            arg0.onpointerout = arg1;
        },
        __wbg_set_onpointerout_5ec372720e2dd041: function(arg0, arg1) {
            arg0.onpointerout = arg1;
        },
        __wbg_set_onpointerout_91111bd12f9de9c2: function(arg0, arg1) {
            arg0.onpointerout = arg1;
        },
        __wbg_set_onpointerover_25c8f8395ba14c87: function(arg0, arg1) {
            arg0.onpointerover = arg1;
        },
        __wbg_set_onpointerover_875ca08d3e525e55: function(arg0, arg1) {
            arg0.onpointerover = arg1;
        },
        __wbg_set_onpointerover_cbaf0766ffd0bb48: function(arg0, arg1) {
            arg0.onpointerover = arg1;
        },
        __wbg_set_onpointerup_65ac63a01fa1cbe7: function(arg0, arg1) {
            arg0.onpointerup = arg1;
        },
        __wbg_set_onpointerup_9aeb06e2090f23e0: function(arg0, arg1) {
            arg0.onpointerup = arg1;
        },
        __wbg_set_onpointerup_ec11007c6cc1791d: function(arg0, arg1) {
            arg0.onpointerup = arg1;
        },
        __wbg_set_onpopstate_e6b5ef171971f2d0: function(arg0, arg1) {
            arg0.onpopstate = arg1;
        },
        __wbg_set_onprogress_50def9a5a85649ff: function(arg0, arg1) {
            arg0.onprogress = arg1;
        },
        __wbg_set_onprogress_8e2928099864f35d: function(arg0, arg1) {
            arg0.onprogress = arg1;
        },
        __wbg_set_onprogress_d1328e3c8df15967: function(arg0, arg1) {
            arg0.onprogress = arg1;
        },
        __wbg_set_onratechange_3a89e716c0ab66b6: function(arg0, arg1) {
            arg0.onratechange = arg1;
        },
        __wbg_set_onratechange_786273edae438648: function(arg0, arg1) {
            arg0.onratechange = arg1;
        },
        __wbg_set_onratechange_a8ec45492b9a306e: function(arg0, arg1) {
            arg0.onratechange = arg1;
        },
        __wbg_set_onreadystatechange_a407abe8e1f6fa8d: function(arg0, arg1) {
            arg0.onreadystatechange = arg1;
        },
        __wbg_set_onreset_1b3cafd237a8fde3: function(arg0, arg1) {
            arg0.onreset = arg1;
        },
        __wbg_set_onreset_d66977b310b0aff4: function(arg0, arg1) {
            arg0.onreset = arg1;
        },
        __wbg_set_onreset_e2a6b8bb7de83d7d: function(arg0, arg1) {
            arg0.onreset = arg1;
        },
        __wbg_set_onresize_b2635ba86d6a67bb: function(arg0, arg1) {
            arg0.onresize = arg1;
        },
        __wbg_set_onresize_b332ecf1257c1caf: function(arg0, arg1) {
            arg0.onresize = arg1;
        },
        __wbg_set_onresize_fe849d695065a650: function(arg0, arg1) {
            arg0.onresize = arg1;
        },
        __wbg_set_onresourcetimingbufferfull_75c5f0569eb537b0: function(arg0, arg1) {
            arg0.onresourcetimingbufferfull = arg1;
        },
        __wbg_set_onscroll_824fb3f2285a2ec8: function(arg0, arg1) {
            arg0.onscroll = arg1;
        },
        __wbg_set_onscroll_8e0159c9c6a5daa4: function(arg0, arg1) {
            arg0.onscroll = arg1;
        },
        __wbg_set_onscroll_afb973db3ef65a15: function(arg0, arg1) {
            arg0.onscroll = arg1;
        },
        __wbg_set_onseeked_79337b86f35144ca: function(arg0, arg1) {
            arg0.onseeked = arg1;
        },
        __wbg_set_onseeked_b6444b4820ca3c9e: function(arg0, arg1) {
            arg0.onseeked = arg1;
        },
        __wbg_set_onseeked_ecb6f818d9b805bf: function(arg0, arg1) {
            arg0.onseeked = arg1;
        },
        __wbg_set_onseeking_31b978a3551e8b99: function(arg0, arg1) {
            arg0.onseeking = arg1;
        },
        __wbg_set_onseeking_c56f8789f295fa4a: function(arg0, arg1) {
            arg0.onseeking = arg1;
        },
        __wbg_set_onseeking_db96b02461119a95: function(arg0, arg1) {
            arg0.onseeking = arg1;
        },
        __wbg_set_onselect_181223d191913cb7: function(arg0, arg1) {
            arg0.onselect = arg1;
        },
        __wbg_set_onselect_8817a2759e351797: function(arg0, arg1) {
            arg0.onselect = arg1;
        },
        __wbg_set_onselect_c3c8480c95cffb94: function(arg0, arg1) {
            arg0.onselect = arg1;
        },
        __wbg_set_onselectionchange_bc472b92e5f86d0b: function(arg0, arg1) {
            arg0.onselectionchange = arg1;
        },
        __wbg_set_onselectstart_131d88eeceb08ea0: function(arg0, arg1) {
            arg0.onselectstart = arg1;
        },
        __wbg_set_onselectstart_268851c833072dbf: function(arg0, arg1) {
            arg0.onselectstart = arg1;
        },
        __wbg_set_onselectstart_44bb4154d604e281: function(arg0, arg1) {
            arg0.onselectstart = arg1;
        },
        __wbg_set_onshow_7655ee1c3760587d: function(arg0, arg1) {
            arg0.onshow = arg1;
        },
        __wbg_set_onshow_87136aff0edbfa29: function(arg0, arg1) {
            arg0.onshow = arg1;
        },
        __wbg_set_onshow_f288d17c84fd0c1a: function(arg0, arg1) {
            arg0.onshow = arg1;
        },
        __wbg_set_onstalled_1f33fa4b2135a5bc: function(arg0, arg1) {
            arg0.onstalled = arg1;
        },
        __wbg_set_onstalled_3d915cfc8874c1c8: function(arg0, arg1) {
            arg0.onstalled = arg1;
        },
        __wbg_set_onstalled_f0abe121b1542fe5: function(arg0, arg1) {
            arg0.onstalled = arg1;
        },
        __wbg_set_onstorage_dc553feef50da776: function(arg0, arg1) {
            arg0.onstorage = arg1;
        },
        __wbg_set_onsubmit_d8ea5dc64c8e39a3: function(arg0, arg1) {
            arg0.onsubmit = arg1;
        },
        __wbg_set_onsubmit_f2209b6bc5cf8368: function(arg0, arg1) {
            arg0.onsubmit = arg1;
        },
        __wbg_set_onsubmit_f3c07401b779cd7a: function(arg0, arg1) {
            arg0.onsubmit = arg1;
        },
        __wbg_set_onsuspend_1f6c4ec04be3c9a3: function(arg0, arg1) {
            arg0.onsuspend = arg1;
        },
        __wbg_set_onsuspend_26fd514621f597be: function(arg0, arg1) {
            arg0.onsuspend = arg1;
        },
        __wbg_set_onsuspend_99d92c06b909e1a8: function(arg0, arg1) {
            arg0.onsuspend = arg1;
        },
        __wbg_set_ontimeupdate_90d90cd42ff6efdd: function(arg0, arg1) {
            arg0.ontimeupdate = arg1;
        },
        __wbg_set_ontimeupdate_a873adf9d2aaecb7: function(arg0, arg1) {
            arg0.ontimeupdate = arg1;
        },
        __wbg_set_ontimeupdate_e0b2967f0a681643: function(arg0, arg1) {
            arg0.ontimeupdate = arg1;
        },
        __wbg_set_ontoggle_4c52317c02ea8480: function(arg0, arg1) {
            arg0.ontoggle = arg1;
        },
        __wbg_set_ontoggle_5a8f8e58d7c3e394: function(arg0, arg1) {
            arg0.ontoggle = arg1;
        },
        __wbg_set_ontoggle_66509a96aefdc2b0: function(arg0, arg1) {
            arg0.ontoggle = arg1;
        },
        __wbg_set_ontouchcancel_9a64605cd8c98e13: function(arg0, arg1) {
            arg0.ontouchcancel = arg1;
        },
        __wbg_set_ontouchcancel_c51d1d68ae4ac9e5: function(arg0, arg1) {
            arg0.ontouchcancel = arg1;
        },
        __wbg_set_ontouchcancel_d9cdcb6cc61ad477: function(arg0, arg1) {
            arg0.ontouchcancel = arg1;
        },
        __wbg_set_ontouchend_875bff6e86e797ca: function(arg0, arg1) {
            arg0.ontouchend = arg1;
        },
        __wbg_set_ontouchend_972d6938d78acf47: function(arg0, arg1) {
            arg0.ontouchend = arg1;
        },
        __wbg_set_ontouchend_f17bb61e53d7a51f: function(arg0, arg1) {
            arg0.ontouchend = arg1;
        },
        __wbg_set_ontouchmove_1c7ae9ff49f55f63: function(arg0, arg1) {
            arg0.ontouchmove = arg1;
        },
        __wbg_set_ontouchmove_7884becd6dc3663e: function(arg0, arg1) {
            arg0.ontouchmove = arg1;
        },
        __wbg_set_ontouchmove_b5006a6b4d522e43: function(arg0, arg1) {
            arg0.ontouchmove = arg1;
        },
        __wbg_set_ontouchstart_0b0835c489fff97c: function(arg0, arg1) {
            arg0.ontouchstart = arg1;
        },
        __wbg_set_ontouchstart_9363d0d6dba8c613: function(arg0, arg1) {
            arg0.ontouchstart = arg1;
        },
        __wbg_set_ontouchstart_e995bce0627a8838: function(arg0, arg1) {
            arg0.ontouchstart = arg1;
        },
        __wbg_set_ontransitioncancel_1afadf9125504f52: function(arg0, arg1) {
            arg0.ontransitioncancel = arg1;
        },
        __wbg_set_ontransitioncancel_3f7754ad40b2aa01: function(arg0, arg1) {
            arg0.ontransitioncancel = arg1;
        },
        __wbg_set_ontransitioncancel_af3b0e608bbef765: function(arg0, arg1) {
            arg0.ontransitioncancel = arg1;
        },
        __wbg_set_ontransitionend_1c81e09350919339: function(arg0, arg1) {
            arg0.ontransitionend = arg1;
        },
        __wbg_set_ontransitionend_8dd93e027ea5227a: function(arg0, arg1) {
            arg0.ontransitionend = arg1;
        },
        __wbg_set_ontransitionend_cb9c370d3981d96c: function(arg0, arg1) {
            arg0.ontransitionend = arg1;
        },
        __wbg_set_ontransitionrun_92939207a7cc60fe: function(arg0, arg1) {
            arg0.ontransitionrun = arg1;
        },
        __wbg_set_ontransitionrun_95670adb79702b99: function(arg0, arg1) {
            arg0.ontransitionrun = arg1;
        },
        __wbg_set_ontransitionrun_bfc16f3c8bce08eb: function(arg0, arg1) {
            arg0.ontransitionrun = arg1;
        },
        __wbg_set_ontransitionstart_17feae66c7c4f642: function(arg0, arg1) {
            arg0.ontransitionstart = arg1;
        },
        __wbg_set_ontransitionstart_a6c5211d22dff5ab: function(arg0, arg1) {
            arg0.ontransitionstart = arg1;
        },
        __wbg_set_ontransitionstart_b0aabb7b83f0465b: function(arg0, arg1) {
            arg0.ontransitionstart = arg1;
        },
        __wbg_set_onunload_050b1616e625d6ef: function(arg0, arg1) {
            arg0.onunload = arg1;
        },
        __wbg_set_onvisibilitychange_9fdfe3f7a7d51d13: function(arg0, arg1) {
            arg0.onvisibilitychange = arg1;
        },
        __wbg_set_onvolumechange_25bb1265a38e8fce: function(arg0, arg1) {
            arg0.onvolumechange = arg1;
        },
        __wbg_set_onvolumechange_2a3e9214d0a0ffa1: function(arg0, arg1) {
            arg0.onvolumechange = arg1;
        },
        __wbg_set_onvolumechange_ce362ba081f32de7: function(arg0, arg1) {
            arg0.onvolumechange = arg1;
        },
        __wbg_set_onvrdisplayactivate_c0e98799dd228f3e: function(arg0, arg1) {
            arg0.onvrdisplayactivate = arg1;
        },
        __wbg_set_onvrdisplayconnect_a93cf6be17030038: function(arg0, arg1) {
            arg0.onvrdisplayconnect = arg1;
        },
        __wbg_set_onvrdisplaydeactivate_2a34c59ede53890f: function(arg0, arg1) {
            arg0.onvrdisplaydeactivate = arg1;
        },
        __wbg_set_onvrdisplaydisconnect_cb6086dc47a42452: function(arg0, arg1) {
            arg0.onvrdisplaydisconnect = arg1;
        },
        __wbg_set_onvrdisplaypresentchange_a89508e13c3502d2: function(arg0, arg1) {
            arg0.onvrdisplaypresentchange = arg1;
        },
        __wbg_set_onwaiting_39b6a471aa9c9571: function(arg0, arg1) {
            arg0.onwaiting = arg1;
        },
        __wbg_set_onwaiting_c915fc4c1e06d339: function(arg0, arg1) {
            arg0.onwaiting = arg1;
        },
        __wbg_set_onwaiting_fd9ec780f72481db: function(arg0, arg1) {
            arg0.onwaiting = arg1;
        },
        __wbg_set_onwebkitanimationend_da75c342bdcd6319: function(arg0, arg1) {
            arg0.onwebkitanimationend = arg1;
        },
        __wbg_set_onwebkitanimationend_dcfc610ccaac9192: function(arg0, arg1) {
            arg0.onwebkitanimationend = arg1;
        },
        __wbg_set_onwebkitanimationend_fb70505f306ba505: function(arg0, arg1) {
            arg0.onwebkitanimationend = arg1;
        },
        __wbg_set_onwebkitanimationiteration_1e489bd6aa34b723: function(arg0, arg1) {
            arg0.onwebkitanimationiteration = arg1;
        },
        __wbg_set_onwebkitanimationiteration_c23355534a53ef1a: function(arg0, arg1) {
            arg0.onwebkitanimationiteration = arg1;
        },
        __wbg_set_onwebkitanimationiteration_d0785b210367c221: function(arg0, arg1) {
            arg0.onwebkitanimationiteration = arg1;
        },
        __wbg_set_onwebkitanimationstart_64e135a4b7ddafd1: function(arg0, arg1) {
            arg0.onwebkitanimationstart = arg1;
        },
        __wbg_set_onwebkitanimationstart_653517916fcc064b: function(arg0, arg1) {
            arg0.onwebkitanimationstart = arg1;
        },
        __wbg_set_onwebkitanimationstart_c0e53fef92085db4: function(arg0, arg1) {
            arg0.onwebkitanimationstart = arg1;
        },
        __wbg_set_onwebkittransitionend_110f7b6e451ffb26: function(arg0, arg1) {
            arg0.onwebkittransitionend = arg1;
        },
        __wbg_set_onwebkittransitionend_2cca6c110dfd6305: function(arg0, arg1) {
            arg0.onwebkittransitionend = arg1;
        },
        __wbg_set_onwebkittransitionend_9ac5c686514aa6a9: function(arg0, arg1) {
            arg0.onwebkittransitionend = arg1;
        },
        __wbg_set_onwheel_7b3aa22d43b909f5: function(arg0, arg1) {
            arg0.onwheel = arg1;
        },
        __wbg_set_onwheel_a71bce90e9f05dad: function(arg0, arg1) {
            arg0.onwheel = arg1;
        },
        __wbg_set_onwheel_e729d6cf3aa2896f: function(arg0, arg1) {
            arg0.onwheel = arg1;
        },
        __wbg_set_opener_7c2ebfea869dac92: function() { return handleError(function (arg0, arg1) {
            arg0.opener = arg1;
        }, arguments); },
        __wbg_set_outerHTML_171912fd810082d2: function(arg0, arg1, arg2) {
            arg0.outerHTML = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_outerHeight_b7f881801b13e516: function() { return handleError(function (arg0, arg1) {
            arg0.outerHeight = arg1;
        }, arguments); },
        __wbg_set_outerWidth_f62e0f65cf6b0cdf: function() { return handleError(function (arg0, arg1) {
            arg0.outerWidth = arg1;
        }, arguments); },
        __wbg_set_password_7ffd6b98bdaeb387: function(arg0, arg1, arg2) {
            arg0.password = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_password_941c775666b551fa: function(arg0, arg1, arg2) {
            arg0.password = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_pathname_0deea493591db5c9: function(arg0, arg1, arg2) {
            arg0.pathname = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_pathname_1eee37f36439bee5: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.pathname = getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_pathname_94fd2fa037483bd8: function(arg0, arg1, arg2) {
            arg0.pathname = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_pattern_897064d8dd3ece84: function(arg0, arg1, arg2) {
            arg0.pattern = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_ping_ef2a4770ae486e32: function(arg0, arg1, arg2) {
            arg0.ping = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_placeholder_89475f02a71a9568: function(arg0, arg1, arg2) {
            arg0.placeholder = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_placeholder_ccfb0181cdf67adc: function(arg0, arg1, arg2) {
            arg0.placeholder = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_popoverTargetAction_e07bb34e2c8f880f: function(arg0, arg1, arg2) {
            arg0.popoverTargetAction = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_popoverTargetElement_f94e1c0092cf7c7e: function(arg0, arg1) {
            arg0.popoverTargetElement = arg1;
        },
        __wbg_set_popover_084b3f24efaacbed: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.popover = arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_port_59774b7c1343d7e0: function(arg0, arg1, arg2) {
            arg0.port = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_port_d8c370ba05c78965: function(arg0, arg1, arg2) {
            arg0.port = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_port_e8987fa94b0b3817: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.port = getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_protocol_3f4f47bdeea7c737: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.protocol = getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_protocol_6223a95a661e5c6a: function(arg0, arg1, arg2) {
            arg0.protocol = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_protocol_ae2e8b28a7d1d00a: function(arg0, arg1, arg2) {
            arg0.protocol = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_readOnly_4bd773501c556228: function(arg0, arg1) {
            arg0.readOnly = arg1 !== 0;
        },
        __wbg_set_readOnly_5cafd57288349787: function(arg0, arg1) {
            arg0.readOnly = arg1 !== 0;
        },
        __wbg_set_referrerPolicy_7929e3bbb29757d9: function(arg0, arg1, arg2) {
            arg0.referrerPolicy = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_referrerPolicy_ac202c43bf255bc3: function(arg0, arg1, arg2) {
            arg0.referrerPolicy = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_rel_da8d101b336beeb3: function(arg0, arg1, arg2) {
            arg0.rel = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_required_66f998ea4786bd1e: function(arg0, arg1) {
            arg0.required = arg1 !== 0;
        },
        __wbg_set_required_71df869bdf19fd99: function(arg0, arg1) {
            arg0.required = arg1 !== 0;
        },
        __wbg_set_rev_0e7350e135c59df3: function(arg0, arg1, arg2) {
            arg0.rev = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_rounding_increment_1c1a0a5ed4a556c3: function(arg0, arg1) {
            arg0.roundingIncrement = arg1 >>> 0;
        },
        __wbg_set_rounding_increment_8aeac41ff54723b0: function(arg0, arg1) {
            arg0.roundingIncrement = arg1 >>> 0;
        },
        __wbg_set_rounding_mode_6fe5a7ff8e4c3f8c: function(arg0, arg1) {
            arg0.roundingMode = __wbindgen_enum_RoundingMode[arg1];
        },
        __wbg_set_rounding_mode_88fb1095ddfb81b5: function(arg0, arg1) {
            arg0.roundingMode = __wbindgen_enum_RoundingMode[arg1];
        },
        __wbg_set_rounding_priority_6be42a1d262abb81: function(arg0, arg1) {
            arg0.roundingPriority = __wbindgen_enum_RoundingPriority[arg1];
        },
        __wbg_set_rounding_priority_b6b38a126c4d10f6: function(arg0, arg1) {
            arg0.roundingPriority = __wbindgen_enum_RoundingPriority[arg1];
        },
        __wbg_set_rows_99f03d4a4a70e7cf: function(arg0, arg1) {
            arg0.rows = arg1 >>> 0;
        },
        __wbg_set_screenX_5eba06887a2db572: function() { return handleError(function (arg0, arg1) {
            arg0.screenX = arg1;
        }, arguments); },
        __wbg_set_screenY_a12856436e307c4f: function() { return handleError(function (arg0, arg1) {
            arg0.screenY = arg1;
        }, arguments); },
        __wbg_set_scrollHeight_ca56c238e5c9e72f: function(arg0, arg1) {
            arg0.scrollHeight = arg1;
        },
        __wbg_set_scrollLeft_26cb75e424120f06: function(arg0, arg1) {
            arg0.scrollLeft = arg1;
        },
        __wbg_set_scrollTop_a8e46c99b633c0fa: function(arg0, arg1) {
            arg0.scrollTop = arg1;
        },
        __wbg_set_scrollTop_e81a7df30390f7d5: function(arg0, arg1) {
            arg0.scrollTop = arg1;
        },
        __wbg_set_scrolling_10acf724b5cdff81: function(arg0, arg1, arg2) {
            arg0.scrolling = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_search_27233ad44df1265a: function(arg0, arg1, arg2) {
            arg0.search = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_search_2fc12c4d1bb93c56: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.search = getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_search_c12c3710c7e7761e: function(arg0, arg1, arg2) {
            arg0.search = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_second_ac0cd74acf3aaa19: function(arg0, arg1) {
            arg0.second = __wbindgen_enum_NumericFormat[arg1];
        },
        __wbg_set_seconds_128be82cdf81b442: function(arg0, arg1) {
            arg0.seconds = __wbindgen_enum_DurationTimeUnitStyle[arg1];
        },
        __wbg_set_seconds_a197b4571341cad3: function(arg0, arg1) {
            arg0.seconds = arg1;
        },
        __wbg_set_seconds_display_c9b0ddd33ae56edb: function(arg0, arg1) {
            arg0.secondsDisplay = __wbindgen_enum_DurationUnitDisplay[arg1];
        },
        __wbg_set_selectedStyleSheetSet_aad14f127ef86a69: function(arg0, arg1, arg2) {
            arg0.selectedStyleSheetSet = arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_selectionDirection_91d2b0051978dc81: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.selectionDirection = arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_selectionDirection_f29f13f7f28e49a4: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.selectionDirection = arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_selectionEnd_7f15ad4c3722e135: function() { return handleError(function (arg0, arg1) {
            arg0.selectionEnd = arg1 === Number.MAX_SAFE_INTEGER ? undefined : arg1;
        }, arguments); },
        __wbg_set_selectionEnd_e892ee82b3a03abf: function() { return handleError(function (arg0, arg1) {
            arg0.selectionEnd = arg1 === Number.MAX_SAFE_INTEGER ? undefined : arg1;
        }, arguments); },
        __wbg_set_selectionStart_098d179dd464f4ff: function() { return handleError(function (arg0, arg1) {
            arg0.selectionStart = arg1 === Number.MAX_SAFE_INTEGER ? undefined : arg1;
        }, arguments); },
        __wbg_set_selectionStart_cb679075c40191d9: function() { return handleError(function (arg0, arg1) {
            arg0.selectionStart = arg1 === Number.MAX_SAFE_INTEGER ? undefined : arg1;
        }, arguments); },
        __wbg_set_selectorText_da3f91e5a003697e: function(arg0, arg1, arg2) {
            arg0.selectorText = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_sensitivity_f24d6ff7e83680e6: function(arg0, arg1) {
            arg0.sensitivity = __wbindgen_enum_CollatorSensitivity[arg1];
        },
        __wbg_set_shape_bb6c1820f27cb431: function(arg0, arg1, arg2) {
            arg0.shape = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_sign_display_a7ba717d0c9ed43d: function(arg0, arg1) {
            arg0.signDisplay = __wbindgen_enum_SignDisplay[arg1];
        },
        __wbg_set_size_89632686bf2a5266: function(arg0, arg1) {
            arg0.size = arg1 >>> 0;
        },
        __wbg_set_slot_332eaab3a20f8d71: function(arg0, arg1, arg2) {
            arg0.slot = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_spellcheck_ad35108c6487f498: function(arg0, arg1) {
            arg0.spellcheck = arg1 !== 0;
        },
        __wbg_set_src_776e79760748b742: function(arg0, arg1, arg2) {
            arg0.src = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_src_8c01f6bdebc42db6: function(arg0, arg1, arg2) {
            arg0.src = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_srcdoc_ed1e6e2677d09040: function(arg0, arg1, arg2) {
            arg0.srcdoc = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_stackTraceLimit_6fc79766fc047bc8: function(arg0) {
            Error.stackTraceLimit = arg0;
        },
        __wbg_set_status_7da99132dae62be6: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.status = getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_step_c8ce867c78a2efd3: function(arg0, arg1, arg2) {
            arg0.step = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_style_4304c4c9bd856bea: function(arg0, arg1) {
            arg0.style = __wbindgen_enum_DurationFormatStyle[arg1];
        },
        __wbg_set_style_696e1517065396e4: function(arg0, arg1) {
            arg0.style = __wbindgen_enum_RelativeTimeFormatStyle[arg1];
        },
        __wbg_set_style_b60bf4819f2ab2b9: function(arg0, arg1) {
            arg0.style = __wbindgen_enum_ListFormatStyle[arg1];
        },
        __wbg_set_style_bccd89de49344f93: function(arg0, arg1) {
            arg0.style = __wbindgen_enum_DisplayNamesStyle[arg1];
        },
        __wbg_set_style_d3ab5efe13615fa3: function(arg0, arg1) {
            arg0.style = __wbindgen_enum_NumberFormatStyle[arg1];
        },
        __wbg_set_tabIndex_fb480de039f8406f: function(arg0, arg1) {
            arg0.tabIndex = arg1;
        },
        __wbg_set_target_fd3c9f1f84fc5f53: function(arg0, arg1, arg2) {
            arg0.target = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_textContent_2a822316d8a2310b: function(arg0, arg1, arg2) {
            arg0.textContent = arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_text_0f90235226aafdd1: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.text = getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_time_style_741b3a3f66c0b2c3: function(arg0, arg1) {
            arg0.timeStyle = __wbindgen_enum_DateTimeStyle[arg1];
        },
        __wbg_set_time_zone_87cd17cf55cd7fb0: function(arg0, arg1, arg2) {
            arg0.timeZone = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_time_zone_name_1613ecd5b5ec1675: function(arg0, arg1) {
            arg0.timeZoneName = __wbindgen_enum_TimeZoneNameFormat[arg1];
        },
        __wbg_set_title_bd6edc450bc7efb8: function(arg0, arg1, arg2) {
            arg0.title = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_title_fd20bb68d03d68bc: function(arg0, arg1, arg2) {
            arg0.title = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_trailing_zero_display_0fee6178a3101cc5: function(arg0, arg1) {
            arg0.trailingZeroDisplay = __wbindgen_enum_TrailingZeroDisplay[arg1];
        },
        __wbg_set_trailing_zero_display_584e7b1dc5f192dc: function(arg0, arg1) {
            arg0.trailingZeroDisplay = __wbindgen_enum_TrailingZeroDisplay[arg1];
        },
        __wbg_set_type_0f498b16c80322fa: function(arg0, arg1) {
            arg0.type = __wbindgen_enum_PluralRulesType[arg1];
        },
        __wbg_set_type_3c0ef91d2f4fad21: function(arg0, arg1, arg2) {
            arg0.type = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_type_8dd906541d1c6e30: function(arg0, arg1) {
            arg0.type = __wbindgen_enum_DisplayNamesType[arg1];
        },
        __wbg_set_type_a700d088c461a553: function(arg0, arg1, arg2) {
            arg0.type = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_type_b0a7ca6282575d98: function(arg0, arg1, arg2) {
            arg0.type = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_type_f2ba381718ed1039: function(arg0, arg1, arg2) {
            arg0.type = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_type_f96d8c0e9582c793: function(arg0, arg1) {
            arg0.type = __wbindgen_enum_ListFormatType[arg1];
        },
        __wbg_set_unit_8fd83488f38a13b0: function(arg0, arg1, arg2) {
            arg0.unit = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_unit_display_c22a0c1be7b684e9: function(arg0, arg1) {
            arg0.unitDisplay = __wbindgen_enum_UnitDisplay[arg1];
        },
        __wbg_set_usage_7a88bd73bfc5f770: function(arg0, arg1) {
            arg0.usage = __wbindgen_enum_CollatorUsage[arg1];
        },
        __wbg_set_useMap_7169bdd4100bc5dd: function(arg0, arg1, arg2) {
            arg0.useMap = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_use_grouping_1e85405078b57ca5: function(arg0, arg1) {
            arg0.useGrouping = __wbindgen_enum_UseGrouping[arg1];
        },
        __wbg_set_username_2a59b9b2880800fe: function(arg0, arg1, arg2) {
            arg0.username = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_username_97d4549ee8355698: function(arg0, arg1, arg2) {
            arg0.username = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_valueAsDate_c93ccfc459f5e702: function() { return handleError(function (arg0, arg1) {
            arg0.valueAsDate = arg1;
        }, arguments); },
        __wbg_set_valueAsNumber_9cea3bdf50267b97: function(arg0, arg1) {
            arg0.valueAsNumber = arg1;
        },
        __wbg_set_value_545fe56298a96d77: function(arg0, arg1) {
            arg0.value = arg1;
        },
        __wbg_set_value_7fcf46f20123a28a: function(arg0, arg1, arg2) {
            arg0.value = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_value_fc1a37d10af775a0: function(arg0, arg1, arg2) {
            arg0.value = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_webkitdirectory_ce178dd09fac1363: function(arg0, arg1) {
            arg0.webkitdirectory = arg1 !== 0;
        },
        __wbg_set_weekday_01794b9ed84c6eb4: function(arg0, arg1) {
            arg0.weekday = __wbindgen_enum_WeekdayFormat[arg1];
        },
        __wbg_set_weeks_18219cd037d92ac3: function(arg0, arg1) {
            arg0.weeks = arg1;
        },
        __wbg_set_weeks_338a1c150a059848: function(arg0, arg1) {
            arg0.weeks = __wbindgen_enum_DurationUnitStyle[arg1];
        },
        __wbg_set_weeks_display_324094063457096c: function(arg0, arg1) {
            arg0.weeksDisplay = __wbindgen_enum_DurationUnitDisplay[arg1];
        },
        __wbg_set_width_1f2e5bfdb8d23b4e: function(arg0, arg1, arg2) {
            arg0.width = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_width_8025bbaa22958d24: function(arg0, arg1) {
            arg0.width = arg1 >>> 0;
        },
        __wbg_set_width_87301412247f3343: function(arg0, arg1) {
            arg0.width = arg1 >>> 0;
        },
        __wbg_set_width_f80e433e57489df3: function(arg0, arg1) {
            arg0.width = arg1;
        },
        __wbg_set_wrap_ebd28bb5bf89e1d9: function(arg0, arg1, arg2) {
            arg0.wrap = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_x_cf44250d18af8c71: function(arg0, arg1) {
            arg0.x = arg1;
        },
        __wbg_set_y_bbe2ff6ad3f3253f: function(arg0, arg1) {
            arg0.y = arg1;
        },
        __wbg_set_year_e40fae254bd6972e: function(arg0, arg1) {
            arg0.year = __wbindgen_enum_YearFormat[arg1];
        },
        __wbg_set_years_337d0e6ea9f2937c: function(arg0, arg1) {
            arg0.years = arg1;
        },
        __wbg_set_years_8d3b5fea86ff97b1: function(arg0, arg1) {
            arg0.years = __wbindgen_enum_DurationUnitStyle[arg1];
        },
        __wbg_set_years_display_53ebbda86cc84485: function(arg0, arg1) {
            arg0.yearsDisplay = __wbindgen_enum_DurationUnitDisplay[arg1];
        },
        __wbg_shape_1ee3f699bc5dd4cd: function(arg0, arg1) {
            const ret = arg1.shape;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_share_0380bb8f7daf332c: function(arg0) {
            const ret = arg0.share();
            return ret;
        },
        __wbg_sheet_fd15cff74ff3b5d2: function(arg0) {
            const ret = arg0.sheet;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_shiftKey_05941b44ffe0a9ce: function(arg0) {
            const ret = arg0.shiftKey;
            return ret;
        },
        __wbg_shiftKey_ec95aec36c86fb31: function(arg0) {
            const ret = arg0.shiftKey;
            return ret;
        },
        __wbg_showPicker_ea4dc3f9fc363010: function() { return handleError(function (arg0) {
            arg0.showPicker();
        }, arguments); },
        __wbg_showPopover_025876d5fd205504: function() { return handleError(function (arg0) {
            arg0.showPopover();
        }, arguments); },
        __wbg_sign_2773ebd42622fc85: function(arg0) {
            const ret = Math.sign(arg0);
            return ret;
        },
        __wbg_sin_b11ddc98e4fe6824: function(arg0) {
            const ret = Math.sin(arg0);
            return ret;
        },
        __wbg_sinh_e9bde9fdf465699e: function(arg0) {
            const ret = Math.sinh(arg0);
            return ret;
        },
        __wbg_size_9970092b88b1094c: function(arg0) {
            const ret = arg0.size;
            return ret;
        },
        __wbg_size_be0a4dae4c805fcb: function(arg0) {
            const ret = arg0.size;
            return ret;
        },
        __wbg_slice_02bb778501725738: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.slice(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_slice_08ba7f74514ecd63: function(arg0, arg1) {
            const ret = arg0.slice(arg1 >>> 0);
            return ret;
        },
        __wbg_slice_15d7d0e9e30dd381: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.slice(arg1, arg2, getStringFromWasm0(arg3, arg4));
            return ret;
        }, arguments); },
        __wbg_slice_1702c8e49a802827: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_19136f942225eb95: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_1ae3833c008e684d: function(arg0, arg1) {
            const ret = arg0.slice(arg1 >>> 0);
            return ret;
        },
        __wbg_slice_1ce28ee0df5e3a42: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.slice(arg1, arg2, getStringFromWasm0(arg3, arg4));
            return ret;
        }, arguments); },
        __wbg_slice_1eba85fb42fe80b3: function(arg0, arg1) {
            const ret = arg0.slice(arg1);
            return ret;
        },
        __wbg_slice_262c269372d7dfb5: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_2c81d3419e9a0836: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_2dae4ca445c72666: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.slice(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_slice_2e0c1dc39bbcc8b2: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.slice(arg1);
            return ret;
        }, arguments); },
        __wbg_slice_43ab3d54dcdb8517: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.slice(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_slice_474b1469cd35b31e: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_4d34106631095b12: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_4d41d852c3fa7fec: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_5ec2b60c616e7d92: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.slice(arg1, arg2, getStringFromWasm0(arg3, arg4));
            return ret;
        }, arguments); },
        __wbg_slice_6a74812965eacc37: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_97b8e270d1fac7b8: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_b517f5fd69b64040: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_c532a1fda73953b8: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_c87a896d40083a6c: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_cf6ba4e5b1341b15: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_dbdafe621cf28e87: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.slice(arg1, arg2, getStringFromWasm0(arg3, arg4));
            return ret;
        }, arguments); },
        __wbg_slice_de6e3666af1c70ad: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_dfa58954db2cd753: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.slice(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_slice_f2cc5bbcc0071b45: function(arg0, arg1, arg2) {
            const ret = arg0.slice(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_slice_f7653d81f247e7c1: function() { return handleError(function (arg0) {
            const ret = arg0.slice();
            return ret;
        }, arguments); },
        __wbg_slice_fe201213f4c7fb9e: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.slice(arg1);
            return ret;
        }, arguments); },
        __wbg_slice_from_f69de4edc432241c: function(arg0, arg1) {
            const ret = arg0.slice_from(arg1);
            return ret;
        },
        __wbg_slot_20948aee6e7f35b4: function(arg0, arg1) {
            const ret = arg1.slot;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_source_45e66e1dff9166de: function(arg0) {
            const ret = arg0.source;
            return ret;
        },
        __wbg_source_8a9bfd9910439b12: function(arg0) {
            const ret = arg0.source;
            return (__wbindgen_enum_RangeSource.indexOf(ret) + 1 || 4) - 1;
        },
        __wbg_source_b7e204e7f348cfab: function(arg0) {
            const ret = arg0.source;
            return (__wbindgen_enum_RangeSource.indexOf(ret) + 1 || 4) - 1;
        },
        __wbg_source_e22e76c585d2cb71: function(arg0) {
            const ret = arg0.source;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_species_da49dbc0be391a2f: function() {
            const ret = Symbol.species;
            return ret;
        },
        __wbg_spellcheck_cb5dacf8bd3aa5e9: function(arg0) {
            const ret = arg0.spellcheck;
            return ret;
        },
        __wbg_splitText_17452e532dc22911: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.splitText(arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_split_4d7508e8714d52b9: function(arg0, arg1, arg2) {
            const ret = arg0.split(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_split_883a3eaeefba6dd6: function(arg0, arg1, arg2) {
            const ret = arg0.split(arg1, arg2 >>> 0);
            return ret;
        },
        __wbg_split_ebf77b3a701ea618: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.split(getStringFromWasm0(arg1, arg2), arg3 >>> 0);
            return ret;
        },
        __wbg_split_f1d177fd0ddde68c: function(arg0, arg1) {
            const ret = arg0.split(arg1);
            return ret;
        },
        __wbg_split_f87674dd17b0b060: function() {
            const ret = Symbol.split;
            return ret;
        },
        __wbg_sqrt_78e1f2bf8b538320: function(arg0) {
            const ret = Math.sqrt(arg0);
            return ret;
        },
        __wbg_src_2f56f7a6a0849189: function(arg0, arg1) {
            const ret = arg1.src;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_src_66e5093fbb1610a2: function(arg0, arg1) {
            const ret = arg1.src;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_srcdoc_b8a6b97edc40ef16: function(arg0, arg1) {
            const ret = arg1.srcdoc;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_stackTraceLimit_b3aa29bd43f5d4d4: function() {
            const ret = Error.stackTraceLimit;
            return ret;
        },
        __wbg_startsWith_b6819cd5f62160e8: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.startsWith(getStringFromWasm0(arg1, arg2), arg3 >>> 0);
            return ret;
        },
        __wbg_state_7824bbeefbc301b1: function() { return handleError(function (arg0) {
            const ret = arg0.state;
            return ret;
        }, arguments); },
        __wbg_state_c4a3c57477740112: function(arg0) {
            const ret = arg0.state;
            return ret;
        },
        __wbg_static_accessor_GLOBAL_9d53f2689e622ca1: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_THIS_a1a35cec07001a8a: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_PI_d046675b670121c1: function() {
            const ret = Math.PI;
            return ret;
        },
        __wbg_static_accessor_SELF_4c59f6c7ea29a144: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_e70ae9f2eb052253: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_statusText_298cf4711928d327: function(arg0, arg1) {
            const ret = arg1.statusText;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_status_00549d55b78d949e: function(arg0) {
            const ret = arg0.status;
            return ret;
        },
        __wbg_status_870ef6e68d4a69bb: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.status;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_step_89101c5af907b553: function(arg0, arg1) {
            const ret = arg1.step;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_sticky_e976aa0f13c4a52e: function(arg0) {
            const ret = arg0.sticky;
            return ret;
        },
        __wbg_stopImmediatePropagation_ea52eb7a070d20d4: function(arg0) {
            arg0.stopImmediatePropagation();
        },
        __wbg_stopPropagation_053e327e3b5c701c: function(arg0) {
            arg0.stopPropagation();
        },
        __wbg_stop_f750a74df2928616: function() { return handleError(function (arg0) {
            arg0.stop();
        }, arguments); },
        __wbg_stringify_1fe8099e9bf72c91: function() { return handleError(function (arg0, arg1) {
            const ret = JSON.stringify(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_stringify_8286df6dcc591521: function() { return handleError(function (arg0) {
            const ret = JSON.stringify(arg0);
            return ret;
        }, arguments); },
        __wbg_stringify_bf7a1bfb07addf05: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            var v0 = getArrayJsValueFromWasm0(arg1, arg2).slice();
            wasm.__wbindgen_free(arg1, arg2 * 4, 4);
            const ret = JSON.stringify(arg0, v0, arg3 === Number.MAX_SAFE_INTEGER ? undefined : arg3);
            return ret;
        }, arguments); },
        __wbg_stringify_dad4f5a7e0c6ffb5: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = JSON.stringify(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_style_4291b02b3de4faea: function(arg0) {
            const ret = arg0.style;
            return ret;
        },
        __wbg_style_ad0f3eb1fd1aa2bc: function(arg0) {
            const ret = arg0.style;
            return ret;
        },
        __wbg_subarray_0ac12a946adaeb7b: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_subarray_1a710c4bd4560bee: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_subarray_1b8425a6c25ca903: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_subarray_1e1d1864ca383e2c: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_subarray_236bc54d71384eb4: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_subarray_240f99514f1a7fb6: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_subarray_4aa221f6a4f5ab22: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_subarray_58bd9101ea67740b: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_subarray_5cb3e4315b59c43f: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_subarray_98f4844a7bddae1b: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_subarray_d4aee526488a9a17: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_subarray_e91d37b003c38512: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_substr_b195f664e9b9c8d9: function(arg0, arg1, arg2) {
            const ret = arg0.substr(arg1, arg2);
            return ret;
        },
        __wbg_substringData_887862b4ef60bedc: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg1.substringData(arg2 >>> 0, arg3 >>> 0);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_substring_082ab4897f3b8354: function(arg0, arg1, arg2) {
            const ret = arg0.substring(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_supportedLocalesOf_299ee1b582844b99: function(arg0, arg1) {
            const ret = Intl.ListFormat.supportedLocalesOf(arg0, arg1);
            return ret;
        },
        __wbg_supportedLocalesOf_3626420acff1f8af: function(arg0, arg1) {
            const ret = Intl.DisplayNames.supportedLocalesOf(arg0, arg1);
            return ret;
        },
        __wbg_supportedLocalesOf_5a77c4b0328f8109: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Intl.DurationFormat.supportedLocalesOf(getArrayJsValueViewFromWasm0(arg0, arg1), arg2);
            return ret;
        }, arguments); },
        __wbg_supportedLocalesOf_6d1869bc93347d60: function(arg0, arg1) {
            const ret = Intl.NumberFormat.supportedLocalesOf(arg0, arg1);
            return ret;
        },
        __wbg_supportedLocalesOf_85cf72ba4b29d68d: function(arg0, arg1) {
            const ret = Intl.Collator.supportedLocalesOf(arg0, arg1);
            return ret;
        },
        __wbg_supportedLocalesOf_920d7e1679c39484: function(arg0, arg1) {
            const ret = Intl.Segmenter.supportedLocalesOf(arg0, arg1);
            return ret;
        },
        __wbg_supportedLocalesOf_9ef570486419000e: function(arg0, arg1) {
            const ret = Intl.RelativeTimeFormat.supportedLocalesOf(arg0, arg1);
            return ret;
        },
        __wbg_supportedLocalesOf_cbb51bb3a3dbc3ae: function(arg0, arg1) {
            const ret = Intl.PluralRules.supportedLocalesOf(arg0, arg1);
            return ret;
        },
        __wbg_supportedLocalesOf_d86202175d0ad2c6: function(arg0, arg1) {
            const ret = Intl.DateTimeFormat.supportedLocalesOf(arg0, arg1);
            return ret;
        },
        __wbg_supportedValuesOf_3f73664b31ee3fa3: function(arg0) {
            const ret = Intl.supportedValuesOf(__wbindgen_enum_SupportedValuesKey[arg0]);
            return ret;
        },
        __wbg_tabIndex_e7c1633f01c9cd5a: function(arg0) {
            const ret = arg0.tabIndex;
            return ret;
        },
        __wbg_table_0a714207d4b07a30: function(arg0, arg1, arg2, arg3) {
            console.table(arg0, arg1, arg2, arg3);
        },
        __wbg_table_40b2ebd8e7f8b618: function(arg0) {
            console.table(arg0);
        },
        __wbg_table_5b5c785fb6657261: function(arg0, arg1) {
            console.table(arg0, arg1);
        },
        __wbg_table_628c5d26e66d05d5: function() {
            console.table();
        },
        __wbg_table_9e704590c8222357: function(arg0) {
            console.table(...arg0);
        },
        __wbg_table_b28275af57e270e8: function(arg0, arg1, arg2) {
            console.table(arg0, arg1, arg2);
        },
        __wbg_table_e57e09dc4fa53c23: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.table(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_table_fc1ed050f777e378: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.table(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_table_fd38b85ba361113f: function(arg0, arg1, arg2, arg3, arg4) {
            console.table(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_tagName_45bb7a95d4c5377b: function(arg0, arg1) {
            const ret = arg1.tagName;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_taintEnabled_aff098afd70bacfb: function(arg0) {
            const ret = arg0.taintEnabled();
            return ret;
        },
        __wbg_tan_2f326db75b2b8391: function(arg0) {
            const ret = Math.tan(arg0);
            return ret;
        },
        __wbg_tangentialPressure_86b5261e93500a5e: function(arg0) {
            const ret = arg0.tangentialPressure;
            return ret;
        },
        __wbg_tanh_eaa85d5e17b53473: function(arg0) {
            const ret = Math.tanh(arg0);
            return ret;
        },
        __wbg_target_44d8e9229ba11d79: function(arg0, arg1) {
            const ret = arg1.target;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_target_baf3e983dceee053: function(arg0) {
            const ret = arg0.target;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_test_1c117f3e94cc8f24: function(arg0, arg1, arg2) {
            const ret = arg0.test(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_textContent_666b896f07901078: function(arg0, arg1) {
            const ret = arg1.textContent;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_textLength_50226017bba30d74: function(arg0) {
            const ret = arg0.textLength;
            return ret;
        },
        __wbg_text_a17febec76d36501: function() { return handleError(function (arg0) {
            const ret = arg0.text();
            return ret;
        }, arguments); },
        __wbg_text_cce67697df0db7ac: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.text;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_text_d886e88139eefda5: function(arg0) {
            const ret = arg0.text();
            return ret;
        },
        __wbg_then_47213a40b6aeb86c: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbg_then_529ea37d9bdbf95d: function(arg0, arg1, arg2) {
            const ret = arg0.then(arg1, arg2);
            return ret;
        },
        __wbg_then_ac7b025999b52837: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbg_tiltX_8d7111f4d9133ef2: function(arg0) {
            const ret = arg0.tiltX;
            return ret;
        },
        __wbg_tiltY_47632da38d059bfe: function(arg0) {
            const ret = arg0.tiltY;
            return ret;
        },
        __wbg_timeEnd_4e2fbae40393374b: function() {
            console.timeEnd();
        },
        __wbg_timeEnd_b96b41372751f5a9: function(arg0, arg1) {
            console.timeEnd(getStringFromWasm0(arg0, arg1));
        },
        __wbg_timeLog_4db2f2c3e16d9539: function() {
            console.timeLog();
        },
        __wbg_timeLog_52ef38d5a1ecc09d: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7) {
            console.timeLog(getStringFromWasm0(arg0, arg1), arg2, arg3, arg4, arg5, arg6, arg7);
        },
        __wbg_timeLog_54056a48bdbc5eac: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.timeLog(getStringFromWasm0(arg0, arg1), arg2, arg3, arg4, arg5);
        },
        __wbg_timeLog_59f0738ff13cb9ce: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8) {
            console.timeLog(getStringFromWasm0(arg0, arg1), arg2, arg3, arg4, arg5, arg6, arg7, arg8);
        },
        __wbg_timeLog_705b62888f3fa356: function(arg0, arg1, arg2, arg3) {
            console.timeLog(getStringFromWasm0(arg0, arg1), arg2, arg3);
        },
        __wbg_timeLog_83e2f9973140b987: function(arg0, arg1) {
            console.timeLog(getStringFromWasm0(arg0, arg1));
        },
        __wbg_timeLog_a1b28cb8133416c9: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.timeLog(getStringFromWasm0(arg0, arg1), arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_timeLog_c407975884cacc36: function(arg0, arg1, arg2) {
            console.timeLog(getStringFromWasm0(arg0, arg1), ...arg2);
        },
        __wbg_timeLog_e89cb55d7e8f2261: function(arg0, arg1, arg2, arg3, arg4) {
            console.timeLog(getStringFromWasm0(arg0, arg1), arg2, arg3, arg4);
        },
        __wbg_timeLog_f0e35104c6d2edba: function(arg0, arg1, arg2) {
            console.timeLog(getStringFromWasm0(arg0, arg1), arg2);
        },
        __wbg_timeOrigin_6fd34a20511c63f5: function(arg0) {
            const ret = arg0.timeOrigin;
            return ret;
        },
        __wbg_timeStamp_2d81565c3e4aa9b8: function() {
            console.timeStamp();
        },
        __wbg_timeStamp_6f4ffda735ee0280: function(arg0) {
            const ret = arg0.timeStamp;
            return ret;
        },
        __wbg_timeStamp_f0c3a2688bfda41f: function(arg0) {
            console.timeStamp(arg0);
        },
        __wbg_time_7ae415e3b61ee688: function(arg0, arg1) {
            console.time(getStringFromWasm0(arg0, arg1));
        },
        __wbg_time_d8adaa9d73184997: function() {
            console.time();
        },
        __wbg_title_19386dd25ef45e29: function(arg0, arg1) {
            const ret = arg1.title;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_title_42afd6249a172b6d: function(arg0, arg1) {
            const ret = arg1.title;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_title_5cd363d68d6cca8b: function(arg0, arg1) {
            const ret = arg1.title;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_toBlob_adb7dbae3cc2f8b0: function() { return handleError(function (arg0, arg1) {
            arg0.toBlob(arg1);
        }, arguments); },
        __wbg_toBlob_daddbc1ed5338f9d: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.toBlob(arg1, getStringFromWasm0(arg2, arg3), arg4);
        }, arguments); },
        __wbg_toBlob_e6ef3234429f23a4: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.toBlob(arg1, getStringFromWasm0(arg2, arg3));
        }, arguments); },
        __wbg_toDataURL_196109be93f1be2d: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg1.toDataURL(getStringFromWasm0(arg2, arg3));
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_toDataURL_9f8d498b14f64e9d: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.toDataURL();
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_toDataURL_d23d9cf0203d5e43: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg1.toDataURL(getStringFromWasm0(arg2, arg3), arg4);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_toDateString_e0ebd4c0b44d3189: function(arg0) {
            const ret = arg0.toDateString();
            return ret;
        },
        __wbg_toExponential_99d9c0e6e0058331: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.toExponential(arg1);
            return ret;
        }, arguments); },
        __wbg_toFixed_a161583a3fd57939: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.toFixed(arg1);
            return ret;
        }, arguments); },
        __wbg_toISOString_d485be0388a74494: function(arg0) {
            const ret = arg0.toISOString();
            return ret;
        },
        __wbg_toJSON_1e2bd8be89226421: function(arg0) {
            const ret = arg0.toJSON();
            return ret;
        },
        __wbg_toJSON_943b9994c6858471: function(arg0) {
            const ret = arg0.toJSON();
            return ret;
        },
        __wbg_toJSON_c47e5cd093d210dc: function(arg0, arg1) {
            const ret = arg1.toJSON();
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_toJSON_d4b02cdf018adb82: function(arg0) {
            const ret = arg0.toJSON();
            return ret;
        },
        __wbg_toLocaleDateString_f27c78a1a3b6801b: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.toLocaleDateString(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        },
        __wbg_toLocaleLowerCase_fa4e569d8480d2e9: function(arg0, arg1, arg2) {
            const ret = arg0.toLocaleLowerCase(arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_toLocaleString_bd930d853054b396: function(arg0, arg1, arg2) {
            const ret = arg0.toLocaleString(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_toLocaleString_dcc47ec85fd77b08: function(arg0, arg1, arg2) {
            const ret = arg0.toLocaleString(arg1, arg2);
            return ret;
        },
        __wbg_toLocaleString_eee54c064312d8d1: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.toLocaleString(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        },
        __wbg_toLocaleTimeString_21b4091bcf6f486c: function(arg0, arg1, arg2) {
            const ret = arg0.toLocaleTimeString(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_toLocaleTimeString_554cf27b85c329b9: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.toLocaleTimeString(getStringFromWasm0(arg1, arg2), arg3);
            return ret;
        },
        __wbg_toLocaleUpperCase_472b7140f2e90967: function(arg0, arg1, arg2) {
            const ret = arg0.toLocaleUpperCase(arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_toLowerCase_9ed222e92ab07d76: function(arg0) {
            const ret = arg0.toLowerCase();
            return ret;
        },
        __wbg_toPrecision_dc7d4091653a4a63: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.toPrecision(arg1);
            return ret;
        }, arguments); },
        __wbg_toPrimitive_b122b0b2d24a319e: function() {
            const ret = Symbol.toPrimitive;
            return ret;
        },
        __wbg_toStringTag_2ed7035408425509: function() {
            const ret = Symbol.toStringTag;
            return ret;
        },
        __wbg_toString_0fa9821f840aaf09: function(arg0, arg1, arg2) {
            const ret = arg1.toString(arg2);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_toString_314f52f9348ffae8: function(arg0) {
            const ret = arg0.toString();
            return ret;
        },
        __wbg_toString_4401cd1f4ebd71be: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.toString(arg1);
            return ret;
        }, arguments); },
        __wbg_toString_812d95db5c6146b2: function(arg0) {
            const ret = arg0.toString();
            return ret;
        },
        __wbg_toString_94ab8ae06f7372cf: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.toString(arg1);
            return ret;
        }, arguments); },
        __wbg_toString_b1f4ccebcd84d602: function(arg0) {
            const ret = arg0.toString();
            return ret;
        },
        __wbg_toString_ccd1ad1d84763afc: function(arg0) {
            const ret = arg0.toString();
            return ret;
        },
        __wbg_toString_ceaa3435e9077b9a: function(arg0) {
            const ret = arg0.toString();
            return ret;
        },
        __wbg_toString_cff1b9c1847d2c58: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.toString(arg1);
            return ret;
        }, arguments); },
        __wbg_toString_e70b7842bff5d893: function(arg0) {
            const ret = arg0.toString();
            return ret;
        },
        __wbg_toTimeString_05f0e4af5bda5d46: function(arg0) {
            const ret = arg0.toTimeString();
            return ret;
        },
        __wbg_toUTCString_e8c6bb63433352bd: function(arg0) {
            const ret = arg0.toUTCString();
            return ret;
        },
        __wbg_toUpperCase_75e192ef642cc317: function(arg0) {
            const ret = arg0.toUpperCase();
            return ret;
        },
        __wbg_toggleAttribute_4b37427d1dbea0bc: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.toggleAttribute(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_toggleAttribute_ec222c903ce67dea: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.toggleAttribute(getStringFromWasm0(arg1, arg2), arg3 !== 0);
            return ret;
        }, arguments); },
        __wbg_togglePopover_9f92bacf10b6a2d8: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.togglePopover(arg1 !== 0);
            return ret;
        }, arguments); },
        __wbg_togglePopover_aef70c75115125c3: function() { return handleError(function (arg0) {
            const ret = arg0.togglePopover();
            return ret;
        }, arguments); },
        __wbg_top_0b769c92cf5b0b27: function() { return handleError(function (arg0) {
            const ret = arg0.top;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_top_14d766e5bde56568: function(arg0) {
            const ret = arg0.top;
            return ret;
        },
        __wbg_trace_0451f5a3bf3fd767: function() {
            console.trace();
        },
        __wbg_trace_148589b27cedf995: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.trace(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_trace_1e6421e03da388c8: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.trace(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_trace_51219d49603a17b0: function(arg0, arg1, arg2) {
            console.trace(arg0, arg1, arg2);
        },
        __wbg_trace_7f56143d6d646949: function(arg0) {
            console.trace(arg0);
        },
        __wbg_trace_b6c1bedad6c868f4: function(arg0, arg1, arg2, arg3, arg4) {
            console.trace(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_trace_b88d746c40d26278: function(arg0) {
            console.trace(...arg0);
        },
        __wbg_trace_d6ceac4ce16072f6: function(arg0, arg1, arg2, arg3) {
            console.trace(arg0, arg1, arg2, arg3);
        },
        __wbg_trace_fbcaf90b4afa87f4: function(arg0, arg1) {
            console.trace(arg0, arg1);
        },
        __wbg_transferToFixedLength_b9001325a097c904: function() { return handleError(function (arg0) {
            const ret = arg0.transferToFixedLength();
            return ret;
        }, arguments); },
        __wbg_transferToFixedLength_d53611bffa4641d3: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.transferToFixedLength(arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_transfer_69bf5c2511044722: function() { return handleError(function (arg0) {
            const ret = arg0.transfer();
            return ret;
        }, arguments); },
        __wbg_transfer_9a68976cad9cbdab: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.transfer(arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_trimEnd_d229bfbe185ef983: function(arg0) {
            const ret = arg0.trimEnd();
            return ret;
        },
        __wbg_trimLeft_2c834eaaf5c3d9dc: function(arg0) {
            const ret = arg0.trimLeft();
            return ret;
        },
        __wbg_trimRight_adb15a3dfbd7f319: function(arg0) {
            const ret = arg0.trimRight();
            return ret;
        },
        __wbg_trimStart_68846f80f85187ff: function(arg0) {
            const ret = arg0.trimStart();
            return ret;
        },
        __wbg_trim_26ea393604fd1948: function(arg0) {
            const ret = arg0.trim();
            return ret;
        },
        __wbg_trunc_6a92f8bdbdb58aa7: function(arg0) {
            const ret = Math.trunc(arg0);
            return ret;
        },
        __wbg_twist_86e2236ace768bb1: function(arg0) {
            const ret = arg0.twist;
            return ret;
        },
        __wbg_type_6488255feec94876: function(arg0, arg1) {
            const ret = arg1.type;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_type_9b4485f164454713: function(arg0) {
            const ret = arg0.type;
            return ret;
        },
        __wbg_type__2da922b40b9e2349: function(arg0) {
            const ret = arg0.type;
            return (__wbindgen_enum_DurationFormatPartType.indexOf(ret) + 1 || 15) - 1;
        },
        __wbg_type__b0e2d1ebd1c0fa72: function(arg0) {
            const ret = arg0.type;
            return (__wbindgen_enum_ListFormatPartType.indexOf(ret) + 1 || 3) - 1;
        },
        __wbg_type__c9022a6143880f2c: function(arg0) {
            const ret = arg0.type;
            return (__wbindgen_enum_DateTimeFormatPartType.indexOf(ret) + 1 || 15) - 1;
        },
        __wbg_type__f0bf8b071bb185db: function(arg0) {
            const ret = arg0.type;
            return (__wbindgen_enum_RelativeTimeFormatPartType.indexOf(ret) + 1 || 5) - 1;
        },
        __wbg_type__fe09b7653e1f088d: function(arg0) {
            const ret = arg0.type;
            return (__wbindgen_enum_NumberFormatPartType.indexOf(ret) + 1 || 18) - 1;
        },
        __wbg_type_c3e1389d9730adf1: function(arg0, arg1) {
            const ret = arg1.type;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_type_d2c93d2bc26d9b55: function(arg0, arg1) {
            const ret = arg1.type;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_type_d7deeb3c0f0fc91c: function(arg0, arg1) {
            const ret = arg1.type;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_type_dafc2005e56c7d7a: function(arg0, arg1) {
            const ret = arg1.type;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_type_e11f70099029338e: function(arg0, arg1) {
            const ret = arg1.type;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_type_f97e3f39d26c07d8: function(arg0, arg1) {
            const ret = arg1.type;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_unescape_740823333b161356: function(arg0, arg1) {
            const ret = unescape(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_unicode_5cad82105f1d7dba: function(arg0) {
            const ret = arg0.unicode;
            return ret;
        },
        __wbg_unit_b80858d3c47e8180: function(arg0) {
            const ret = arg0.unit;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_unit_e848bdb432ce112a: function(arg0) {
            const ret = arg0.unit;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_unobserve_dc7a4a975eb22fc2: function(arg0, arg1) {
            arg0.unobserve(arg1);
        },
        __wbg_unregister_ddf7425c4ffec2a6: function(arg0, arg1) {
            const ret = arg0.unregister(arg1);
            return ret;
        },
        __wbg_unscopables_25c02e68340a1764: function() {
            const ret = Symbol.unscopables;
            return ret;
        },
        __wbg_url_6808f1c468f2d0cd: function(arg0, arg1) {
            const ret = arg1.url;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_url_8b9d120d9dc02d8f: function(arg0, arg1) {
            const ret = arg1.url;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_useMap_dd7ba85ddf25c83c: function(arg0, arg1) {
            const ret = arg1.useMap;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_userAgent_8def8135d886414b: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.userAgent;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_username_c227112115b3d28d: function(arg0, arg1) {
            const ret = arg1.username;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_username_fb1618d1df0ff704: function(arg0, arg1) {
            const ret = arg1.username;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_validate_46315018d277d0e2: function() { return handleError(function (arg0) {
            const ret = WebAssembly.validate(arg0);
            return ret;
        }, arguments); },
        __wbg_validationMessage_5196a7722ba675be: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.validationMessage;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_validationMessage_8aba38866dbf2a3e: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.validationMessage;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_valueAsDate_9aa23e6d2b2a86cc: function() { return handleError(function (arg0) {
            const ret = arg0.valueAsDate;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_valueAsNumber_625471e2fa71e6d8: function(arg0) {
            const ret = arg0.valueAsNumber;
            return ret;
        },
        __wbg_valueOf_2663782dd4aab274: function(arg0) {
            const ret = arg0.valueOf();
            return ret;
        },
        __wbg_valueOf_41ae57308c1f031c: function(arg0) {
            const ret = arg0.valueOf();
            return ret;
        },
        __wbg_valueOf_acd1c96f47ea43a8: function(arg0) {
            const ret = arg0.valueOf();
            return ret;
        },
        __wbg_valueOf_b2109a2f65eb811e: function(arg0) {
            const ret = arg0.valueOf();
            return ret;
        },
        __wbg_valueOf_b260286ad6421f66: function(arg0, arg1) {
            const ret = arg0.valueOf(arg1);
            return ret;
        },
        __wbg_valueOf_e14ddf6c20f39f9b: function(arg0) {
            const ret = arg0.valueOf();
            return ret;
        },
        __wbg_value_064fd25ee08b7490: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbg_value_6177c7953f900695: function(arg0, arg1) {
            const ret = arg1.value;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_value_a894028fe7eb40cb: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbg_value_e813b4fd6922a0f3: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbg_value_ed244ce0aafd9d66: function(arg0, arg1) {
            const ret = arg1.value;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_value_ef4db0abf06040af: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbg_value_f2b519a3559b2710: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbg_value_fb72578f9c84dc07: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbg_vibrate_1d2d57b4950536b6: function(arg0, arg1) {
            const ret = arg0.vibrate(arg1);
            return ret;
        },
        __wbg_vibrate_3e0883e7993fc04d: function(arg0, arg1) {
            const ret = arg0.vibrate(arg1 >>> 0);
            return ret;
        },
        __wbg_view_d9638b1c583d21e8: function(arg0) {
            const ret = arg0.view;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_waitAsync_17b13db8aebe7c18: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = Atomics.waitAsync(arg0, arg1 >>> 0, arg2, arg3);
            return ret;
        }, arguments); },
        __wbg_waitAsync_1e42de43fa7859a8: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Atomics.waitAsync(arg0, arg1 >>> 0, arg2);
            return ret;
        }, arguments); },
        __wbg_waitAsync_7677bed9a22d36da: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Atomics.waitAsync(arg0, arg1 >>> 0, arg2);
            return ret;
        }, arguments); },
        __wbg_waitAsync_8ccf15b37d3985d7: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = Atomics.waitAsync(arg0, arg1 >>> 0, arg2, arg3);
            return ret;
        }, arguments); },
        __wbg_wait_20d039872aa0f0c8: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Atomics.wait(arg0, arg1 >>> 0, arg2);
            return ret;
        }, arguments); },
        __wbg_wait_2457d921a3f12476: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = Atomics.wait(arg0, arg1 >>> 0, arg2, arg3);
            return ret;
        }, arguments); },
        __wbg_wait_b94396d36b6d3c59: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = Atomics.wait(arg0, arg1 >>> 0, arg2, arg3);
            return ret;
        }, arguments); },
        __wbg_wait_f7e8c75b7e3d0ffa: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Atomics.wait(arg0, arg1 >>> 0, arg2);
            return ret;
        }, arguments); },
        __wbg_warn_0eb3c61f29eec6a1: function() {
            console.warn();
        },
        __wbg_warn_410c3261e3c6d686: function(arg0) {
            console.warn(arg0);
        },
        __wbg_warn_99b743b1ee8a2d9c: function(arg0) {
            console.warn(...arg0);
        },
        __wbg_warn_a2ce2ad871d02328: function(arg0, arg1) {
            console.warn(arg0, arg1);
        },
        __wbg_warn_bfd30614ea2e03d0: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            console.warn(arg0, arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_warn_c49a7a9581bf8bea: function(arg0, arg1, arg2, arg3) {
            console.warn(arg0, arg1, arg2, arg3);
        },
        __wbg_warn_d6eca212db59e84e: function(arg0, arg1, arg2, arg3, arg4) {
            console.warn(arg0, arg1, arg2, arg3, arg4);
        },
        __wbg_warn_e1946f0c4d22a86a: function(arg0, arg1, arg2) {
            console.warn(arg0, arg1, arg2);
        },
        __wbg_warn_f41845c821b346e6: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            console.warn(arg0, arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_wasClean_9636ab9b65f5dbb9: function(arg0) {
            const ret = arg0.wasClean;
            return ret;
        },
        __wbg_webkitEntries_9d3f94d3d4293461: function(arg0) {
            const ret = arg0.webkitEntries;
            return ret;
        },
        __wbg_webkitMatchesSelector_f3ab80363a991f9a: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.webkitMatchesSelector(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_webkitdirectory_56dd72fea5b9c085: function(arg0) {
            const ret = arg0.webkitdirectory;
            return ret;
        },
        __wbg_weekend_c81890dc6d147eba: function(arg0) {
            const ret = arg0.weekend;
            return ret;
        },
        __wbg_weeks_f86dea5ae9fd5099: function(arg0, arg1) {
            const ret = arg1.weeks;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg_which_50a262061fe5a86b: function(arg0) {
            const ret = arg0.which;
            return ret;
        },
        __wbg_wholeText_eff7f56f323a3b59: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.wholeText;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_width_1e0b74fef17bc28b: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbg_width_23a8752e66a3beab: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbg_width_699320f00b42fce1: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbg_width_796e38875beab5e6: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbg_width_c66a869a0bcfbc13: function(arg0, arg1) {
            const ret = arg1.width;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_width_dd13177d91751e3c: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbg_willValidate_23073e0a85f027c2: function(arg0) {
            const ret = arg0.willValidate;
            return ret;
        },
        __wbg_willValidate_9aba7fad53d355b8: function(arg0) {
            const ret = arg0.willValidate;
            return ret;
        },
        __wbg_window_74548839ca59a334: function(arg0) {
            const ret = arg0.window;
            return ret;
        },
        __wbg_wrap_69c1c7035b9bfaa1: function(arg0, arg1) {
            const ret = arg1.wrap;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_x_d04fc448eb267831: function(arg0) {
            const ret = arg0.x;
            return ret;
        },
        __wbg_x_e64ab23f42714230: function(arg0) {
            const ret = arg0.x;
            return ret;
        },
        __wbg_x_eb15ab8d1b665b73: function(arg0) {
            const ret = arg0.x;
            return ret;
        },
        __wbg_y_3b172249a25694a5: function(arg0) {
            const ret = arg0.y;
            return ret;
        },
        __wbg_y_7a4ac1ce3a336bab: function(arg0) {
            const ret = arg0.y;
            return ret;
        },
        __wbg_y_c4a34029ac91ece3: function(arg0) {
            const ret = arg0.y;
            return ret;
        },
        __wbg_years_ef223be0b2b4fee9: function(arg0, arg1) {
            const ret = arg1.years;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref, Externref], shim_idx: 486, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, _ZN12wasm_bindgen7convert8closures1_6invoke17h617c337bf075f06eE);
            return ret;
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 373, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, _ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E);
            return ret;
        },
        __wbindgen_cast_0000000000000003: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 374, ret: Externref, inner_ret: Some(Externref) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, _ZN12wasm_bindgen7convert8closures1_6invoke17h8ff09ad6ee2a4fb9E);
            return ret;
        },
        __wbindgen_cast_0000000000000004: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 481, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, _ZN12wasm_bindgen7convert8closures1_6invoke17h55071bcc8f5249ceE);
            return ret;
        },
        __wbindgen_cast_0000000000000005: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("Event")], shim_idx: 372, ret: Unit, inner_ret: Some(Unit) }, mutable: false }) -> Externref`.
            const ret = makeClosure(arg0, arg1, _ZN12wasm_bindgen7convert8closures1_6invoke17hf1444fba52da8246E);
            return ret;
        },
        __wbindgen_cast_0000000000000006: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("Event")], shim_idx: 373, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, _ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E_5);
            return ret;
        },
        __wbindgen_cast_0000000000000007: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("KeyboardEvent")], shim_idx: 373, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, _ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E_6);
            return ret;
        },
        __wbindgen_cast_0000000000000008: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("MouseEvent")], shim_idx: 373, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, _ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E_7);
            return ret;
        },
        __wbindgen_cast_0000000000000009: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("PointerEvent")], shim_idx: 373, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, _ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E_8);
            return ret;
        },
        __wbindgen_cast_000000000000000a: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [], shim_idx: 370, ret: Externref, inner_ret: Some(Externref) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, _ZN12wasm_bindgen7convert8closures1_6invoke17hb5c8826067030ca0E);
            return ret;
        },
        __wbindgen_cast_000000000000000b: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [], shim_idx: 371, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, _ZN12wasm_bindgen7convert8closures1_6invoke17h69652ca28388492aE);
            return ret;
        },
        __wbindgen_cast_000000000000000c: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_cast_000000000000000d: function(arg0) {
            // Cast intrinsic for `I64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_cast_000000000000000e: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(F32)) -> NamedExternref("Float32Array")`.
            const ret = getArrayF32FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_000000000000000f: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(F64)) -> NamedExternref("Float64Array")`.
            const ret = getArrayF64FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000010: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(I16)) -> NamedExternref("Int16Array")`.
            const ret = getArrayI16FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000011: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(I32)) -> NamedExternref("Int32Array")`.
            const ret = getArrayI32FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000012: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(I64)) -> NamedExternref("BigInt64Array")`.
            const ret = getArrayI64FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000013: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(I8)) -> NamedExternref("Int8Array")`.
            const ret = getArrayI8FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000014: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U16)) -> NamedExternref("Uint16Array")`.
            const ret = getArrayU16FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000015: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U32)) -> NamedExternref("Uint32Array")`.
            const ret = getArrayU32FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000016: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U64)) -> NamedExternref("BigUint64Array")`.
            const ret = getArrayU64FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000017: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U8)) -> NamedExternref("Uint8Array")`.
            const ret = getArrayU8FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000018: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U8)) -> NamedExternref("Uint8ClampedArray")`.
            const ret = getArrayU8FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000019: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_000000000000001a: function(arg0) {
            // Cast intrinsic for `U64 -> Externref`.
            const ret = BigInt.asUintN(64, arg0);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./dynlink_mainweb_bg.js": import0,
    };
}

function __wbg_call_guard() {
    if (__wbg_reinit_scheduled) {
        __wbg_reset_state();
        return;
    }
}
function _ZN12wasm_bindgen7convert8closures1_6invoke17h69652ca28388492aE(arg0, arg1) {
    __wbg_call_guard();
    wasm._ZN12wasm_bindgen7convert8closures1_6invoke17h69652ca28388492aE(arg0, arg1);
}

function _ZN12wasm_bindgen7convert8closures1_6invoke17hb5c8826067030ca0E(arg0, arg1) {
    let ret;
    __wbg_call_guard();
    ret = wasm._ZN12wasm_bindgen7convert8closures1_6invoke17hb5c8826067030ca0E(arg0, arg1);
    return ret;
}

function _ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E(arg0, arg1, arg2) {
    __wbg_call_guard();
    wasm._ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E(arg0, arg1, arg2);
}

function _ZN12wasm_bindgen7convert8closures1_6invoke17hf1444fba52da8246E(arg0, arg1, arg2) {
    __wbg_call_guard();
    wasm._ZN12wasm_bindgen7convert8closures1_6invoke17hf1444fba52da8246E(arg0, arg1, arg2);
}

function _ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E_5(arg0, arg1, arg2) {
    __wbg_call_guard();
    wasm._ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E_5(arg0, arg1, arg2);
}

function _ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E_6(arg0, arg1, arg2) {
    __wbg_call_guard();
    wasm._ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E_6(arg0, arg1, arg2);
}

function _ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E_7(arg0, arg1, arg2) {
    __wbg_call_guard();
    wasm._ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E_7(arg0, arg1, arg2);
}

function _ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E_8(arg0, arg1, arg2) {
    __wbg_call_guard();
    wasm._ZN12wasm_bindgen7convert8closures1_6invoke17h22302698fa9a0598E_8(arg0, arg1, arg2);
}

function _ZN12wasm_bindgen7convert8closures1_6invoke17h8ff09ad6ee2a4fb9E(arg0, arg1, arg2) {
    let ret;
    __wbg_call_guard();
    ret = wasm._ZN12wasm_bindgen7convert8closures1_6invoke17h8ff09ad6ee2a4fb9E(arg0, arg1, arg2);
    return ret;
}

function _ZN12wasm_bindgen7convert8closures1_6invoke17h55071bcc8f5249ceE(arg0, arg1, arg2) {
    let ret;
    __wbg_call_guard();
    ret = wasm._ZN12wasm_bindgen7convert8closures1_6invoke17h55071bcc8f5249ceE(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function _ZN12wasm_bindgen7convert8closures1_6invoke17h617c337bf075f06eE(arg0, arg1, arg2, arg3) {
    __wbg_call_guard();
    wasm._ZN12wasm_bindgen7convert8closures1_6invoke17h617c337bf075f06eE(arg0, arg1, arg2, arg3);
}


const __wbindgen_enum_BinaryType = ["blob", "arraybuffer"];


const __wbindgen_enum_CollatorCaseFirst = ["upper", "lower", "false"];


const __wbindgen_enum_CollatorSensitivity = ["base", "accent", "case", "variant"];


const __wbindgen_enum_CollatorUsage = ["sort", "search"];


const __wbindgen_enum_CompactDisplay = ["short", "long"];


const __wbindgen_enum_CurrencyDisplay = ["code", "symbol", "narrowSymbol", "name"];


const __wbindgen_enum_CurrencySign = ["standard", "accounting"];


const __wbindgen_enum_DateTimeFormatPartType = ["day", "dayPeriod", "era", "fractionalSecond", "hour", "literal", "minute", "month", "relatedYear", "second", "timeZoneName", "weekday", "year", "yearName"];


const __wbindgen_enum_DateTimeStyle = ["full", "long", "medium", "short"];


const __wbindgen_enum_DayFormat = ["numeric", "2-digit"];


const __wbindgen_enum_DayPeriodFormat = ["narrow", "short", "long"];


const __wbindgen_enum_DisplayNamesFallback = ["code", "none"];


const __wbindgen_enum_DisplayNamesLanguageDisplay = ["dialect", "standard"];


const __wbindgen_enum_DisplayNamesStyle = ["long", "short", "narrow"];


const __wbindgen_enum_DisplayNamesType = ["language", "region", "script", "currency", "calendar", "dateTimeField"];


const __wbindgen_enum_DurationFormatPartType = ["years", "months", "weeks", "days", "hours", "minutes", "seconds", "milliseconds", "microseconds", "nanoseconds", "literal", "integer", "decimal", "fraction"];


const __wbindgen_enum_DurationFormatStyle = ["long", "short", "narrow", "digital"];


const __wbindgen_enum_DurationTimeUnitStyle = ["long", "short", "narrow", "numeric", "2-digit"];


const __wbindgen_enum_DurationUnitDisplay = ["auto", "always"];


const __wbindgen_enum_DurationUnitStyle = ["long", "short", "narrow"];


const __wbindgen_enum_EraFormat = ["narrow", "short", "long"];


const __wbindgen_enum_HourCycle = ["h11", "h12", "h23", "h24"];


const __wbindgen_enum_ListFormatPartType = ["element", "literal"];


const __wbindgen_enum_ListFormatStyle = ["long", "short", "narrow"];


const __wbindgen_enum_ListFormatType = ["conjunction", "disjunction", "unit"];


const __wbindgen_enum_LocaleMatcher = ["lookup", "best fit"];


const __wbindgen_enum_MonthFormat = ["numeric", "2-digit", "narrow", "short", "long"];


const __wbindgen_enum_NumberFormatNotation = ["standard", "scientific", "engineering", "compact"];


const __wbindgen_enum_NumberFormatPartType = ["compact", "currency", "decimal", "exponentInteger", "exponentMinusSign", "exponentSeparator", "fraction", "group", "infinity", "integer", "literal", "minusSign", "nan", "percentSign", "plusSign", "unit", "unknown"];


const __wbindgen_enum_NumberFormatStyle = ["decimal", "currency", "percent", "unit"];


const __wbindgen_enum_NumericFormat = ["numeric", "2-digit"];


const __wbindgen_enum_PluralRulesType = ["cardinal", "ordinal"];


const __wbindgen_enum_RangeSource = ["startRange", "endRange", "shared"];


const __wbindgen_enum_RelativeTimeFormatNumeric = ["always", "auto"];


const __wbindgen_enum_RelativeTimeFormatPartType = ["literal", "integer", "decimal", "fraction"];


const __wbindgen_enum_RelativeTimeFormatStyle = ["long", "short", "narrow"];


const __wbindgen_enum_RoundingMode = ["ceil", "floor", "expand", "trunc", "halfCeil", "halfFloor", "halfExpand", "halfTrunc", "halfEven"];


const __wbindgen_enum_RoundingPriority = ["auto", "morePrecision", "lessPrecision"];


const __wbindgen_enum_SegmenterGranularity = ["grapheme", "word", "sentence"];


const __wbindgen_enum_SignDisplay = ["auto", "never", "always", "exceptZero"];


const __wbindgen_enum_SupportedValuesKey = ["calendar", "collation", "currency", "numberingSystem", "timeZone", "unit"];


const __wbindgen_enum_TimeZoneNameFormat = ["short", "long", "shortOffset", "longOffset", "shortGeneric", "longGeneric"];


const __wbindgen_enum_TrailingZeroDisplay = ["auto", "stripIfInteger"];


const __wbindgen_enum_UnitDisplay = ["short", "narrow", "long"];


const __wbindgen_enum_UseGrouping = ["always", "auto", "min2", "true", "false"];


const __wbindgen_enum_WeekdayFormat = ["narrow", "short", "long"];


const __wbindgen_enum_YearFormat = ["numeric", "2-digit"];


let __wbg_instance_id = 0;

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(state => {
    if (state.instance === __wbg_instance_id) {
        wasm.__wbindgen_destroy_closure(state.a, state.b);
    }
});

function debugString(val) {
    // primitive types
    const type = typeof val;
    if (type == 'number' || type == 'boolean' || val == null) {
        return  `${val}`;
    }
    if (type == 'string') {
        return `"${val}"`;
    }
    if (type == 'symbol') {
        const description = val.description;
        if (description == null) {
            return 'Symbol';
        } else {
            return `Symbol(${description})`;
        }
    }
    if (type == 'function') {
        const name = val.name;
        if (typeof name == 'string' && name.length > 0) {
            return `Function(${name})`;
        } else {
            return 'Function';
        }
    }
    // objects
    if (Array.isArray(val)) {
        const length = val.length;
        let debug = '[';
        if (length > 0) {
            debug += debugString(val[0]);
        }
        for(let i = 1; i < length; i++) {
            debug += ', ' + debugString(val[i]);
        }
        debug += ']';
        return debug;
    }
    // Test for built-in
    const builtInMatches = /\[object ([^\]]+)\]/.exec(toString.call(val));
    let className;
    if (builtInMatches && builtInMatches.length > 1) {
        className = builtInMatches[1];
    } else {
        // Failed to match the standard '[object ClassName]'
        return toString.call(val);
    }
    if (className == 'Object') {
        // we're a user defined class or Object
        // JSON.stringify avoids problems with cycles, and is generally much
        // easier than looping through ownProperties of `val`.
        try {
            return 'Object(' + JSON.stringify(val) + ')';
        } catch (_) {
            return 'Object';
        }
    }
    // errors
    if (val instanceof Error) {
        return `${val.name}: ${val.message}\n${val.stack}`;
    }
    // TODO we could test for more things here, like `Set`s and `Map`s.
    return className;
}

function getArrayF32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getFloat32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayF64FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getFloat64ArrayMemory0().subarray(ptr / 8, ptr / 8 + len);
}

function getArrayI16FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getInt16ArrayMemory0().subarray(ptr / 2, ptr / 2 + len);
}

function getArrayI32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getInt32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayI64FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getBigInt64ArrayMemory0().subarray(ptr / 8, ptr / 8 + len);
}

function getArrayI8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getInt8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

function getArrayJsValueFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    const mem = getDataViewMemory0();
    const result = [];
    for (let i = ptr; i < ptr + 4 * len; i += 4) {
        result.push(wasm.__wbindgen_externrefs.get(mem.getUint32(i, true)));
    }
    wasm.__externref_drop_slice(ptr, len);
    return result;
}

function getArrayJsValueViewFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    const mem = getDataViewMemory0();
    const result = [];
    for (let i = ptr; i < ptr + 4 * len; i += 4) {
        result.push(wasm.__wbindgen_externrefs.get(mem.getUint32(i, true)));
    }
    return result;
}

function getArrayU16FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint16ArrayMemory0().subarray(ptr / 2, ptr / 2 + len);
}

function getArrayU32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayU64FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getBigUint64ArrayMemory0().subarray(ptr / 8, ptr / 8 + len);
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedBigInt64ArrayMemory0 = null;
function getBigInt64ArrayMemory0() {
    if (cachedBigInt64ArrayMemory0 === null || cachedBigInt64ArrayMemory0.byteLength === 0) {
        cachedBigInt64ArrayMemory0 = new BigInt64Array(wasm.memory.buffer);
    }
    return cachedBigInt64ArrayMemory0;
}

let cachedBigUint64ArrayMemory0 = null;
function getBigUint64ArrayMemory0() {
    if (cachedBigUint64ArrayMemory0 === null || cachedBigUint64ArrayMemory0.byteLength === 0) {
        cachedBigUint64ArrayMemory0 = new BigUint64Array(wasm.memory.buffer);
    }
    return cachedBigUint64ArrayMemory0;
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

let cachedFloat32ArrayMemory0 = null;
function getFloat32ArrayMemory0() {
    if (cachedFloat32ArrayMemory0 === null || cachedFloat32ArrayMemory0.byteLength === 0) {
        cachedFloat32ArrayMemory0 = new Float32Array(wasm.memory.buffer);
    }
    return cachedFloat32ArrayMemory0;
}

let cachedFloat64ArrayMemory0 = null;
function getFloat64ArrayMemory0() {
    if (cachedFloat64ArrayMemory0 === null || cachedFloat64ArrayMemory0.byteLength === 0) {
        cachedFloat64ArrayMemory0 = new Float64Array(wasm.memory.buffer);
    }
    return cachedFloat64ArrayMemory0;
}

let cachedInt16ArrayMemory0 = null;
function getInt16ArrayMemory0() {
    if (cachedInt16ArrayMemory0 === null || cachedInt16ArrayMemory0.byteLength === 0) {
        cachedInt16ArrayMemory0 = new Int16Array(wasm.memory.buffer);
    }
    return cachedInt16ArrayMemory0;
}

let cachedInt32ArrayMemory0 = null;
function getInt32ArrayMemory0() {
    if (cachedInt32ArrayMemory0 === null || cachedInt32ArrayMemory0.byteLength === 0) {
        cachedInt32ArrayMemory0 = new Int32Array(wasm.memory.buffer);
    }
    return cachedInt32ArrayMemory0;
}

let cachedInt8ArrayMemory0 = null;
function getInt8ArrayMemory0() {
    if (cachedInt8ArrayMemory0 === null || cachedInt8ArrayMemory0.byteLength === 0) {
        cachedInt8ArrayMemory0 = new Int8Array(wasm.memory.buffer);
    }
    return cachedInt8ArrayMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint16ArrayMemory0 = null;
function getUint16ArrayMemory0() {
    if (cachedUint16ArrayMemory0 === null || cachedUint16ArrayMemory0.byteLength === 0) {
        cachedUint16ArrayMemory0 = new Uint16Array(wasm.memory.buffer);
    }
    return cachedUint16ArrayMemory0;
}

let cachedUint32ArrayMemory0 = null;
function getUint32ArrayMemory0() {
    if (cachedUint32ArrayMemory0 === null || cachedUint32ArrayMemory0.byteLength === 0) {
        cachedUint32ArrayMemory0 = new Uint32Array(wasm.memory.buffer);
    }
    return cachedUint32ArrayMemory0;
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function makeClosure(arg0, arg1, f) {
    const state = { a: arg0, b: arg1, cnt: 1, instance: __wbg_instance_id };
    const real = (...args) => {

        if (state.instance !== __wbg_instance_id) {
            throw new Error('Cannot invoke closure from previous WASM instance');
        }

        // First up with a closure we increment the internal reference
        // count. This ensures that the Rust closure environment won't
        // be deallocated while we're invoking it.
        state.cnt++;
        try {
            return f(state.a, state.b, ...args);
        } finally {
            real._wbg_cb_unref();
        }
    };
    real._wbg_cb_unref = () => {
        if (--state.cnt === 0) {
            wasm.__wbindgen_destroy_closure(state.a, state.b);
            state.a = 0;
            CLOSURE_DTORS.unregister(state);
        }
    };
    CLOSURE_DTORS.register(real, state, state);
    return real;
}

function makeMutClosure(arg0, arg1, f) {
    const state = { a: arg0, b: arg1, cnt: 1, instance: __wbg_instance_id };
    const real = (...args) => {

        if (state.instance !== __wbg_instance_id) {
            throw new Error('Cannot invoke closure from previous WASM instance');
        }

        // First up with a closure we increment the internal reference
        // count. This ensures that the Rust closure environment won't
        // be deallocated while we're invoking it.
        state.cnt++;
        const a = state.a;
        state.a = 0;
        try {
            return f(a, state.b, ...args);
        } finally {
            state.a = a;
            real._wbg_cb_unref();
        }
    };
    real._wbg_cb_unref = () => {
        if (--state.cnt === 0) {
            wasm.__wbindgen_destroy_closure(state.a, state.b);
            state.a = 0;
            CLOSURE_DTORS.unregister(state);
        }
    };
    CLOSURE_DTORS.register(real, state, state);
    return real;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

let __wbg_reinit_scheduled = false;

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedBigInt64ArrayMemory0 = null;
    cachedBigUint64ArrayMemory0 = null;
    cachedDataViewMemory0 = null;
    cachedFloat32ArrayMemory0 = null;
    cachedFloat64ArrayMemory0 = null;
    cachedInt16ArrayMemory0 = null;
    cachedInt32ArrayMemory0 = null;
    cachedInt8ArrayMemory0 = null;
    cachedUint16ArrayMemory0 = null;
    cachedUint32ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('dynlink_mainweb_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default, __wbg_get_imports, __wbg_finalize_init };
