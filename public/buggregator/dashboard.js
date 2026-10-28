'use strict';

// Helpers shared by the service dashboard components.
const dash = {
	RANGES : { '6h':21600 , '24h':86400 , '7d':604800 , '30d':2592000 } ,

	// How an instance ended => severity and label. Planned ends are neutral.
	END_REASONS : {
		running : { severity:'good' , label:'Running' } ,
		shutdown : { severity:'neutral' , label:'Planned stop' } ,
		exit : { severity:'neutral' , label:'Finished' } ,
		watchdog : { severity:'warning' , label:'Watchdog' } ,
		memory : { severity:'warning' , label:'Memory self-restart' } ,
		other : { severity:'serious' , label:'Other' } ,
		vanished : { severity:'neutral' , label:'Replaced' } , // new pod, no final report: restart/deploy from outside
		crash : { severity:'critical' , label:'Crash' } ,
		panic : { severity:'critical' , label:'Panic' } ,
		oom : { severity:'critical' , label:'Likely OOM' } ,
	} ,
	SEVERITY_ICONS : { good:'✓' , neutral:'■' , warning:'▲' , serious:'◆' , critical:'✕' } ,
	SEVERITY_RANK : { good:0 , neutral:0 , warning:1 , serious:2 , critical:3 } ,

	STATUS : {
		up : { severity:'good' , label:'Up' } ,
		stale : { severity:'warning' , label:'Stale' } ,
		down : { severity:'critical' , label:'Down' } ,
		stopped : { severity:'serious' , label:'Stopped' } ,
	} ,

	endReason ( reason ) {
		return dash.END_REASONS[reason||'running'] || dash.END_REASONS.other ;
	} ,

	isUnplanned ( reason ) {
		return dash.SEVERITY_RANK[dash.endReason(reason).severity] > 0 ;
	} ,

	api ( params ) {
		return new Promise ( function ( resolve , reject ) {
			$.get ( './api.php' , params , function ( d ) {
				if ( d.status == 'OK' ) resolve ( d.data ) ;
				else reject ( d.status ) ;
			} , 'json' ).fail ( function () { reject ( 'Network error' ) } ) ;
		} ) ;
	} ,

	compact ( v , digits ) {
		if ( v === null || v === undefined || isNaN(v) ) return '–' ;
		let a = Math.abs(v) ;
		if ( a >= 1e9 ) return (v/1e9).toFixed(1) + 'G' ;
		if ( a >= 1e6 ) return (v/1e6).toFixed(1) + 'M' ;
		if ( a >= 1e3 ) return (v/1e3).toFixed(1) + 'K' ;
		if ( a >= 100 || v == Math.round(v) ) return Math.round(v).toString() ;
		return v.toFixed ( digits === undefined ? (a < 1 ? 2 : 1) : digits ) ;
	} ,

	// unit: bytes | seconds | cores | per_min | percent | count
	format ( v , unit ) {
		if ( v === null || v === undefined || isNaN(v) ) return '–' ;
		if ( unit == 'bytes' ) {
			let units = ['B','KiB','MiB','GiB','TiB'] , i = 0 ;
			while ( Math.abs(v) >= 1024 && i < units.length-1 ) { v /= 1024 ; i++ ; }
			return ( i == 0 || v >= 100 ? Math.round(v) : v.toFixed(1) ) + ' ' + units[i] ;
		}
		if ( unit == 'seconds' ) {
			if ( v < 1 ) return Math.round(v*1000) + ' ms' ;
			if ( v < 120 ) return v.toFixed(v < 10 ? 2 : 1) + ' s' ;
			return dash.duration ( v ) ;
		}
		if ( unit == 'per_min' ) {
			if ( v > 0 && v < 1 ) return dash.compact(v*60) + '/h' ;
			return dash.compact(v) + '/min' ;
		}
		if ( unit == 'percent' ) { // Never round 99.95% up to 100%
			if ( v >= 1 ) return '100%' ;
			return ( Math.floor ( v * ( v >= 0.99 ? 10000 : 1000 ) ) / ( v >= 0.99 ? 100 : 10 ) ) + '%' ;
		}
		if ( unit == 'cores' ) return v.toFixed(2) ;
		return dash.compact ( v ) ;
	} ,

	// "+12%", "−40%", "new" (nothing before), "gone" (nothing now).
	change ( now , before ) {
		if ( !before ) return now ? 'new' : '' ;
		if ( !now ) return 'gone' ;
		let p = Math.round ( ( now - before ) / before * 100 ) ;
		return ( p > 0 ? '+' : p < 0 ? '−' : '±' ) + Math.abs(p) + '%' ;
	} ,

	duration ( seconds ) {
		seconds = Math.max ( 0 , Math.round(seconds) ) ;
		let d = Math.floor(seconds/86400) , h = Math.floor(seconds%86400/3600) , m = Math.floor(seconds%3600/60) ;
		if ( d > 0 ) return d + ' d ' + h + ' h' ;
		if ( h > 0 ) return h + ' h ' + m + ' min' ;
		if ( m > 0 ) return m + ' min' ;
		return seconds + ' s' ;
	} ,

	ago ( ts , now ) {
		if ( !ts ) return '–' ;
		return dash.duration ( (now||Date.now()/1000) - ts ) + ' ago' ;
	} ,

	dateTime ( ts ) {
		if ( !ts ) return '–' ;
		let d = new Date ( ts*1000 ) ;
		let pad = function(n){ return (n<10?'0':'')+n } ;
		return d.getFullYear() + '-' + pad(d.getMonth()+1) + '-' + pad(d.getDate()) + ' ' + pad(d.getHours()) + ':' + pad(d.getMinutes()) ;
	} ,

	// The unit a metric is shown in, from naming conventions.
	unitFor ( name , kind ) {
		if ( /_bytes$/.test(name) ) return 'bytes' ;
		if ( /_seconds(_p50|_p95|_max)?$/.test(name) ) return 'seconds' ;
		if ( kind == 'counter' ) return 'per_min' ;
		return 'count' ;
	} ,

	labelText ( labels ) {
		let parts = [] ;
		$.each ( labels || {} , function ( k , v ) { parts.push ( k + '=' + v ) } ) ;
		return parts.join ( ', ' ) ;
	} ,
} ;
